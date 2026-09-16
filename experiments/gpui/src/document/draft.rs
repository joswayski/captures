use super::{
    BlendMode, Document, Layer, Shape,
    geometry::{Point, Rect},
};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, RgbaImage};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const SCHEMA_VERSION: u16 = 1;
const MAX_LAYERS: usize = 64;
const MAX_TOTAL_ASSET_BYTES: usize = 80 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 16_384;
const MAX_IMAGE_PIXELS: u64 = 100_000_000;
const MANIFEST: &str = "manifest.json";
const CURRENT: &str = "current";
const PREVIOUS: &str = "previous";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DraftIdentity {
    Capture { path: PathBuf },
    Imported { path: PathBuf },
    NewCapture { id: String },
}

impl DraftIdentity {
    pub fn capture(path: PathBuf) -> Self {
        Self::Capture { path }
    }

    pub fn imported(path: PathBuf) -> Self {
        Self::Imported { path }
    }

    pub fn new_capture() -> Self {
        Self::NewCapture {
            id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub fn source_path(&self) -> Option<&Path> {
        match self {
            Self::Capture { path } | Self::Imported { path } => Some(path),
            Self::NewCapture { .. } => None,
        }
    }

    pub fn matches_source_path(&self, path: &Path) -> bool {
        self.source_path()
            .is_some_and(|source| normalized_path(source) == normalized_path(path))
    }

    pub fn same_storage_location(&self, other: &Self) -> bool {
        self.key() == other.key()
    }

    fn key(&self) -> String {
        let mut hash = 0xcbf29ce484222325_u64;
        let mut feed = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        };
        match self {
            Self::Capture { path } => {
                feed(b"capture\0");
                feed(normalized_path(path).as_bytes());
                format!("capture-{hash:016x}")
            }
            Self::Imported { path } => {
                feed(b"imported\0");
                feed(normalized_path(path).as_bytes());
                format!("imported-{hash:016x}")
            }
            Self::NewCapture { id } => {
                feed(b"new-capture\0");
                feed(id.as_bytes());
                format!("new-{hash:016x}")
            }
        }
    }
}

fn normalized_path(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value
            .strip_prefix("//?/")
            .unwrap_or(&value)
            .to_ascii_lowercase()
    } else {
        value
    }
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    schema_version: u16,
    identity: DraftIdentity,
    source_path: Option<PathBuf>,
    updated_at_ms: u64,
    document: DocumentData,
}

#[derive(Serialize, Deserialize)]
struct DocumentData {
    original_asset: String,
    source_asset: String,
    source_present: bool,
    source_visible: bool,
    source_name: String,
    crop: Rect,
    canvas_width: u32,
    canvas_height: u32,
    background: Option<[u8; 4]>,
    layers: Vec<LayerData>,
    next_id: u64,
}

#[derive(Serialize, Deserialize)]
struct LayerData {
    id: u64,
    name: String,
    #[serde(default)]
    background: bool,
    shape: ShapeData,
    original_pixels_asset: Option<String>,
    color: [u8; 4],
    stroke: f32,
    fill: Option<[u8; 4]>,
    opacity: u8,
    blend_mode: BlendModeData,
    rotation_degrees: f32,
    visible: bool,
    locked: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ShapeData {
    Stroke {
        points: Vec<Point>,
    },
    Arrow {
        start: Point,
        end: Point,
    },
    Line {
        start: Point,
        end: Point,
    },
    Rectangle {
        rect: Rect,
    },
    Ellipse {
        rect: Rect,
    },
    Polygon {
        points: Vec<Point>,
    },
    Image {
        origin: Point,
        width: f32,
        height: f32,
        asset: String,
    },
    Text {
        origin: Point,
        value: String,
        font_size: f32,
        font_asset: String,
        #[serde(default)]
        font: Option<TextFontData>,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        italic: bool,
        #[serde(default)]
        align: TextAlignData,
        #[serde(default)]
        width: Option<f32>,
        #[serde(default)]
        background: Option<[u8; 4]>,
        #[serde(default)]
        rounded_background: bool,
        #[serde(default)]
        outlined: bool,
        #[serde(default)]
        shadow: Option<TextShadowData>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
struct TextFontData {
    family: String,
    collection_index: u32,
    bold: bool,
    italic: bool,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TextAlignData {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Serialize, Deserialize)]
struct TextShadowData {
    color: [u8; 4],
    blur: f32,
    offset: Point,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BlendModeData {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
}

pub struct DraftStore {
    root: PathBuf,
}

pub struct DraftSession {
    pub identity: DraftIdentity,
    pub persisted_key: (u64, u64),
    pub observed_key: (u64, u64),
    pub changed_at: Option<Instant>,
    pub in_flight: Option<(u64, u64)>,
}

pub fn retire_capture_session(session: &mut Option<DraftSession>, capture_path: &Path) -> bool {
    let capture = DraftIdentity::capture(capture_path.to_path_buf());
    if session
        .as_ref()
        .is_some_and(|session| session.identity.same_storage_location(&capture))
    {
        *session = None;
        true
    } else {
        false
    }
}

impl DraftStore {
    pub fn new(profile_root: &Path) -> Self {
        Self {
            root: profile_root.join("screenshot-editor-drafts"),
        }
    }

    pub fn save(
        &self,
        identity: &DraftIdentity,
        source_path: Option<&Path>,
        document: &Document,
    ) -> Result<(), String> {
        self.save_with_asset_limit(identity, source_path, document, MAX_TOTAL_ASSET_BYTES)
    }

    fn save_with_asset_limit(
        &self,
        identity: &DraftIdentity,
        source_path: Option<&Path>,
        document: &Document,
        asset_limit: usize,
    ) -> Result<(), String> {
        if document.layers.len() > MAX_LAYERS {
            return Err("this edit has too many layers to keep as a draft".into());
        }
        let directory = self.root.join(identity.key());
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let staging = directory.join(format!(".staging-{}", uuid::Uuid::new_v4()));
        let assets_dir = staging.join("assets");
        fs::create_dir_all(&assets_dir).map_err(|error| error.to_string())?;
        let mut assets = AssetWriter::new(&assets_dir, asset_limit);
        let result = (|| {
            let original_asset = assets.image("original", &document.original)?;
            let source_asset = if Arc::ptr_eq(&document.original, &document.source) {
                original_asset.clone()
            } else {
                assets.image("source", &document.source)?
            };
            let mut layers = Vec::with_capacity(document.layers.len());
            for layer in &document.layers {
                layers.push(LayerData::from_layer(layer, &mut assets)?);
            }
            let manifest = Manifest {
                schema_version: SCHEMA_VERSION,
                identity: identity.clone(),
                source_path: source_path.map(Path::to_path_buf),
                updated_at_ms: now_ms(),
                document: DocumentData {
                    original_asset,
                    source_asset,
                    source_present: document.source_present,
                    source_visible: document.source_visible,
                    source_name: document.source_name.clone(),
                    crop: document.crop,
                    canvas_width: document.canvas_width,
                    canvas_height: document.canvas_height,
                    background: document.background,
                    layers,
                    next_id: document.next_id,
                },
            };
            let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
            if manifest_bytes.len() as u64 > MAX_MANIFEST_BYTES {
                return Err("screenshot draft manifest exceeds the allowed size".into());
            }
            atomic_write(&staging.join(MANIFEST), &manifest_bytes)?;
            publish_snapshot(&directory, &staging)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    pub fn load(
        &self,
        identity: &DraftIdentity,
        source_path: Option<&Path>,
    ) -> Result<Option<Document>, String> {
        let directory = self.root.join(identity.key());
        let Some(snapshot) = readable_snapshot(&directory)? else {
            return Ok(None);
        };
        let bytes = read_capped(&snapshot.join(MANIFEST), MAX_MANIFEST_BYTES)?;
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if manifest.schema_version != SCHEMA_VERSION {
            return Ok(None);
        }
        let source_matches = match (manifest.source_path.as_deref(), source_path) {
            (Some(saved), Some(requested)) => normalized_path(saved) == normalized_path(requested),
            (None, None) => true,
            _ => false,
        };
        if manifest.identity.key() != identity.key() || !source_matches {
            return Err(
                "screenshot draft source identity does not match the requested image".into(),
            );
        }
        manifest
            .document
            .into_document(&snapshot.join("assets"))
            .map(Some)
    }

    pub fn discard(&self, identity: &DraftIdentity) -> Result<(), String> {
        let directory = self.root.join(identity.key());
        match fs::remove_dir_all(directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

struct AssetWriter<'a> {
    directory: &'a Path,
    total: usize,
    limit: usize,
}

impl<'a> AssetWriter<'a> {
    fn new(directory: &'a Path, limit: usize) -> Self {
        Self {
            directory,
            total: 0,
            limit,
        }
    }

    fn image(&mut self, id: &str, image: &RgbaImage) -> Result<String, String> {
        validate_image_dimensions(image.width(), image.height())?;
        let mut cursor = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image.clone())
            .write_to(&mut cursor, ImageFormat::Png)
            .map_err(|error| error.to_string())?;
        self.bytes(id, "png", &cursor.into_inner())
    }

    fn font(&mut self, id: &str, bytes: &[u8]) -> Result<String, String> {
        self.bytes(id, "font", bytes)
    }

    fn bytes(&mut self, id: &str, extension: &str, bytes: &[u8]) -> Result<String, String> {
        self.total = self.total.saturating_add(bytes.len());
        if self.total > self.limit {
            return Err("unsaved edits are too large to keep as a draft; save a file first".into());
        }
        let name = format!("{id}.{extension}");
        atomic_write(&self.directory.join(&name), bytes)?;
        Ok(name)
    }
}

impl LayerData {
    fn from_layer(layer: &Layer, assets: &mut AssetWriter<'_>) -> Result<Self, String> {
        let shape = match &layer.shape {
            Shape::Stroke(points) => ShapeData::Stroke {
                points: points.clone(),
            },
            Shape::Arrow(start, end) => ShapeData::Arrow {
                start: *start,
                end: *end,
            },
            Shape::Line(start, end) => ShapeData::Line {
                start: *start,
                end: *end,
            },
            Shape::Rectangle(rect) => ShapeData::Rectangle { rect: *rect },
            Shape::Ellipse(rect) => ShapeData::Ellipse { rect: *rect },
            Shape::Polygon(points) => ShapeData::Polygon {
                points: points.clone(),
            },
            Shape::Image {
                origin,
                width,
                height,
                pixels,
            } => ShapeData::Image {
                origin: *origin,
                width: *width,
                height: *height,
                asset: assets.image(&format!("layer-{}", layer.id), pixels)?,
            },
            Shape::Text {
                origin,
                value,
                font_size,
                font_data,
                style,
            } => ShapeData::Text {
                origin: *origin,
                value: value.clone(),
                font_size: *font_size,
                font_asset: assets.font(&format!("layer-{}", layer.id), font_data)?,
                font: style.font.as_ref().map(|font| TextFontData {
                    family: font.family.clone(),
                    collection_index: font.collection_index,
                    bold: font.bold,
                    italic: font.italic,
                }),
                bold: style.bold,
                italic: style.italic,
                align: match style.align {
                    captures_image::TextAlign::Left => TextAlignData::Left,
                    captures_image::TextAlign::Center => TextAlignData::Center,
                    captures_image::TextAlign::Right => TextAlignData::Right,
                },
                width: style.width,
                background: style.background,
                rounded_background: style.rounded_background,
                outlined: style.outlined,
                shadow: style.shadow.as_ref().map(|shadow| TextShadowData {
                    color: shadow.color,
                    blur: shadow.blur,
                    offset: Point {
                        x: shadow.offset.x,
                        y: shadow.offset.y,
                    },
                }),
            },
        };
        let original_pixels_asset = layer
            .original_pixels
            .as_ref()
            .map(|pixels| assets.image(&format!("layer-{}-original", layer.id), pixels))
            .transpose()?;
        Ok(Self {
            id: layer.id,
            name: layer.name.clone(),
            background: layer.background,
            shape,
            original_pixels_asset,
            color: layer.color,
            stroke: layer.stroke,
            fill: layer.fill,
            opacity: layer.opacity,
            blend_mode: layer.blend_mode.into(),
            rotation_degrees: layer.rotation_degrees,
            visible: layer.visible,
            locked: layer.locked,
        })
    }

    fn into_layer(self, assets: &Path) -> Result<Layer, String> {
        let shape = match self.shape {
            ShapeData::Stroke { points } => Shape::Stroke(points),
            ShapeData::Arrow { start, end } => Shape::Arrow(start, end),
            ShapeData::Line { start, end } => Shape::Line(start, end),
            ShapeData::Rectangle { rect } => Shape::Rectangle(rect),
            ShapeData::Ellipse { rect } => Shape::Ellipse(rect),
            ShapeData::Polygon { points } => Shape::Polygon(points),
            ShapeData::Image {
                origin,
                width,
                height,
                asset,
            } => Shape::Image {
                origin,
                width,
                height,
                pixels: Arc::new(read_image(&asset_path(assets, &asset)?)?),
            },
            ShapeData::Text {
                origin,
                value,
                font_size,
                font_asset,
                font,
                bold,
                italic,
                align,
                width,
                background,
                rounded_background,
                outlined,
                shadow,
            } => Shape::Text {
                origin,
                value,
                font_size,
                font_data: fs::read(asset_path(assets, &font_asset)?)
                    .map_err(|e| e.to_string())?
                    .into(),
                style: captures_image::TextStyleSettings {
                    font: font.map(|font| captures_image::TextFont {
                        family: font.family,
                        collection_index: font.collection_index,
                        bold: font.bold,
                        italic: font.italic,
                    }),
                    bold,
                    italic,
                    align: match align {
                        TextAlignData::Left => captures_image::TextAlign::Left,
                        TextAlignData::Center => captures_image::TextAlign::Center,
                        TextAlignData::Right => captures_image::TextAlign::Right,
                    },
                    width,
                    background,
                    rounded_background,
                    outlined,
                    shadow: shadow.map(|shadow| captures_image::TextShadow {
                        color: shadow.color,
                        blur: shadow.blur,
                        offset: captures_image::Point {
                            x: shadow.offset.x,
                            y: shadow.offset.y,
                        },
                    }),
                },
            },
        };
        Ok(Layer {
            id: self.id,
            name: self.name,
            background: self.background,
            shape,
            original_pixels: self
                .original_pixels_asset
                .map(|asset| {
                    asset_path(assets, &asset)
                        .and_then(|path| read_image(&path))
                        .map(Arc::new)
                })
                .transpose()?,
            color: self.color,
            stroke: self.stroke,
            fill: self.fill,
            opacity: self.opacity,
            blend_mode: self.blend_mode.into(),
            rotation_degrees: self.rotation_degrees,
            visible: self.visible,
            locked: self.locked,
        })
    }
}

impl DocumentData {
    fn into_document(self, assets: &Path) -> Result<Document, String> {
        if self.canvas_width == 0
            || self.canvas_height == 0
            || self.canvas_width > MAX_IMAGE_DIMENSION
            || self.canvas_height > MAX_IMAGE_DIMENSION
            || u64::from(self.canvas_width) * u64::from(self.canvas_height) > MAX_IMAGE_PIXELS
            || self.layers.len() > MAX_LAYERS
        {
            return Err("screenshot draft dimensions or layer count are invalid".into());
        }
        validate_assets(assets, self.asset_names())?;
        let original = Arc::new(read_image(&asset_path(assets, &self.original_asset)?)?);
        let source = Arc::new(read_image(&asset_path(assets, &self.source_asset)?)?);
        let layers = self
            .layers
            .into_iter()
            .map(|layer| layer.into_layer(assets))
            .collect::<Result<Vec<_>, _>>()?;
        let next_id = self
            .next_id
            .max(layers.iter().map(|layer| layer.id).max().unwrap_or(0) + 1);
        Ok(Document {
            id: super::NEXT_DOCUMENT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            original,
            source,
            source_present: self.source_present,
            source_visible: self.source_visible,
            source_name: self.source_name,
            crop: self.crop,
            canvas_width: self.canvas_width,
            canvas_height: self.canvas_height,
            background: self.background,
            layers,
            undo: Vec::new(),
            redo: Vec::new(),
            next_id,
            revision: 1,
        })
    }

    fn asset_names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.original_asset.as_str())
            .chain(std::iter::once(self.source_asset.as_str()))
            .chain(self.layers.iter().flat_map(|layer| {
                let shape = match &layer.shape {
                    ShapeData::Image { asset, .. } => Some(asset.as_str()),
                    ShapeData::Text { font_asset, .. } => Some(font_asset.as_str()),
                    _ => None,
                };
                shape
                    .into_iter()
                    .chain(layer.original_pixels_asset.as_deref())
            }))
    }
}

impl From<BlendMode> for BlendModeData {
    fn from(value: BlendMode) -> Self {
        match value {
            BlendMode::Normal => Self::Normal,
            BlendMode::Multiply => Self::Multiply,
            BlendMode::Screen => Self::Screen,
            BlendMode::Overlay => Self::Overlay,
            BlendMode::Darken => Self::Darken,
            BlendMode::Lighten => Self::Lighten,
        }
    }
}

impl From<BlendModeData> for BlendMode {
    fn from(value: BlendModeData) -> Self {
        match value {
            BlendModeData::Normal => Self::Normal,
            BlendModeData::Multiply => Self::Multiply,
            BlendModeData::Screen => Self::Screen,
            BlendModeData::Overlay => Self::Overlay,
            BlendModeData::Darken => Self::Darken,
            BlendModeData::Lighten => Self::Lighten,
        }
    }
}

fn read_image(path: &Path) -> Result<RgbaImage, String> {
    let reader = ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let (width, height) = decoder.dimensions();
    validate_image_dimensions(width, height)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_PIXELS * 4 + 64 * 1024 * 1024);
    decoder
        .set_limits(limits)
        .map_err(|error| error.to_string())?;
    DynamicImage::from_decoder(decoder)
        .map(|image| image.to_rgba8())
        .map_err(|error| error.to_string())
}

fn validate_image_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        Err("screenshot draft image dimensions are invalid".into())
    } else {
        Ok(())
    }
}

fn validate_assets<'a>(assets: &Path, names: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let mut unique = HashSet::new();
    let mut total = 0_u64;
    for name in names {
        let path = asset_path(assets, name)?;
        if unique.insert(name) {
            let length = fs::metadata(path).map_err(|error| error.to_string())?.len();
            total = total.saturating_add(length);
            if total > MAX_TOTAL_ASSET_BYTES as u64 {
                return Err("screenshot draft assets exceed the allowed size".into());
            }
        }
    }
    Ok(())
}

fn read_capped(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let length = fs::metadata(path).map_err(|error| error.to_string())?.len();
    if length > maximum {
        return Err("screenshot draft manifest exceeds the allowed size".into());
    }
    fs::read(path).map_err(|error| error.to_string())
}

fn readable_snapshot(directory: &Path) -> Result<Option<PathBuf>, String> {
    for name in [CURRENT, PREVIOUS] {
        let snapshot = directory.join(name);
        match fs::metadata(snapshot.join(MANIFEST)) {
            Ok(metadata) if metadata.is_file() => return Ok(Some(snapshot)),
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(None)
}

fn publish_snapshot(directory: &Path, staging: &Path) -> Result<(), String> {
    let current = directory.join(CURRENT);
    let previous = directory.join(PREVIOUS);
    let had_current = current.exists();
    if had_current {
        if previous.exists() {
            fs::remove_dir_all(&previous).map_err(|error| error.to_string())?;
        }
        fs::rename(&current, &previous).map_err(|error| error.to_string())?;
    }
    match fs::rename(staging, &current) {
        Ok(()) => {
            if previous.exists() {
                let _ = fs::remove_dir_all(previous);
            }
            Ok(())
        }
        Err(error) => {
            if had_current {
                let _ = fs::rename(previous, current);
            }
            Err(error.to_string())
        }
    }
}

fn asset_path(directory: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || name.len() > 96
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("screenshot draft contains an invalid asset name".into());
    }
    Ok(directory.join(name))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("draft path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let file_name = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("invalid draft filename")?;
    let staging = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&staging).map_err(|error| error.to_string())?;
    let result = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&staging);
        return Err(error.to_string());
    }
    let backup = parent.join(format!(".{file_name}.{}.bak", uuid::Uuid::new_v4()));
    let had_previous = path.exists();
    if had_previous {
        fs::rename(path, &backup).map_err(|error| error.to_string())?;
    }
    match fs::rename(&staging, path) {
        Ok(()) => {
            if had_previous {
                let _ = fs::remove_file(backup);
            }
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&staging);
            if had_previous {
                let _ = fs::rename(backup, path);
            }
            Err(error.to_string())
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renamed_background_metadata_survives_restart_and_old_layers_default() {
        let profile = tempfile::tempdir().unwrap();
        let identity = DraftIdentity::new_capture();
        let store = DraftStore::new(profile.path());
        let mut document = Document::new(asymmetric_source());
        let id = document.materialize_source(true).unwrap();
        document.rename_layer(id, "Reference".into());
        document.add_image(asymmetric_source(), 0, "Inset".into());
        store.save(&identity, None, &document).unwrap();
        let restored = store.load(&identity, None).unwrap().unwrap();
        assert!(restored.layers[0].background);
        assert!(restored.layers[0].locked);
        assert_eq!(restored.layers[0].name, "Reference");
        assert!(!restored.layers[1].background);

        let path = store.root.join(identity.key()).join(CURRENT).join(MANIFEST);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for layer in manifest["document"]["layers"].as_array_mut().unwrap() {
            layer.as_object_mut().unwrap().remove("background");
        }
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let legacy = store.load(&identity, None).unwrap().unwrap();
        assert!(legacy.layers.iter().all(|layer| !layer.background));
        assert_eq!(legacy.render().unwrap(), restored.render().unwrap());
    }

    #[test]
    fn styled_text_roundtrips_and_old_text_defaults_remain_compatible() {
        let profile = tempfile::tempdir().unwrap();
        let identity = DraftIdentity::new_capture();
        let store = DraftStore::new(profile.path());
        let mut document = Document::new(RgbaImage::from_pixel(32, 24, Rgba([0, 0, 0, 0])));
        document.add(
            Shape::Text {
                origin: Point { x: 2.0, y: 3.0 },
                value: "styled".into(),
                font_size: 14.0,
                font_data: Arc::from(vec![1, 2, 3]),
                style: captures_image::TextStyleSettings {
                    font: Some(captures_image::TextFont {
                        family: "mono".into(),
                        collection_index: 3,
                        bold: true,
                        italic: false,
                    }),
                    bold: true,
                    italic: true,
                    align: captures_image::TextAlign::Right,
                    width: Some(120.0),
                    background: Some([4, 5, 6, 220]),
                    rounded_background: true,
                    outlined: true,
                    shadow: Some(captures_image::TextShadow {
                        color: [1, 2, 3, 100],
                        blur: 7.0,
                        offset: captures_image::Point { x: 3.0, y: 5.0 },
                    }),
                },
            },
            [255, 255, 255, 255],
            0.0,
        );
        store.save(&identity, None, &document).unwrap();
        let loaded = store.load(&identity, None).unwrap().unwrap();
        assert_eq!(loaded.layers[0].shape, document.layers[0].shape);

        let manifest_path = store.root.join(identity.key()).join(CURRENT).join(MANIFEST);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        let shape = &mut manifest["document"]["layers"][0]["shape"];
        for key in [
            "font",
            "bold",
            "italic",
            "align",
            "width",
            "background",
            "rounded_background",
            "outlined",
            "shadow",
        ] {
            shape.as_object_mut().unwrap().remove(key);
        }
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let old = store.load(&identity, None).unwrap().unwrap();
        let Shape::Text { style, .. } = &old.layers[0].shape else {
            panic!("text")
        };
        assert_eq!(style, &captures_image::TextStyleSettings::default());
    }
    use image::Rgba;

    fn asymmetric_source() -> RgbaImage {
        RgbaImage::from_fn(7, 5, |x, y| {
            Rgba([
                (13 + x * 23) as u8,
                (17 + y * 31) as u8,
                (x * 11 + y * 7) as u8,
                255,
            ])
        })
    }

    #[test]
    fn restart_restores_editable_raster_layers_without_reading_or_rewriting_source() {
        let profile = tempfile::tempdir().unwrap();
        let source_path = profile.path().join("outside-source.png");
        fs::write(&source_path, b"source bytes must remain untouched").unwrap();
        let identity = DraftIdentity::imported(source_path.clone());
        let mut document = Document::new(asymmetric_source());
        document.set_crop(Rect {
            x: 1.0,
            y: 1.0,
            width: 5.0,
            height: 3.0,
        });
        document.set_background(Some([3, 5, 7, 255]));
        let mut pixels = RgbaImage::from_pixel(3, 2, Rgba([211, 41, 73, 220]));
        pixels.put_pixel(2, 1, Rgba([19, 181, 223, 151]));
        let image_id = document.add_image(pixels, 0, "Editable import.png".into());
        document.set_layer_rotation(image_id, -17.0);
        document.set_layer_opacity(image_id, 173);
        let layer = document
            .layers
            .iter_mut()
            .find(|layer| layer.id == image_id)
            .unwrap();
        let Shape::Image { pixels, .. } = &mut layer.shape else {
            panic!("expected imported image layer");
        };
        layer.original_pixels = Some(pixels.clone());
        let mut erased = pixels.as_ref().clone();
        erased.put_pixel(0, 0, Rgba([211, 41, 73, 0]));
        *pixels = Arc::new(erased);
        let rectangle_id = document.add(
            Shape::Rectangle(Rect {
                x: 2.0,
                y: 1.0,
                width: 2.0,
                height: 1.0,
            }),
            [29, 101, 233, 255],
            2.0,
        );
        document.set_layer_fill(rectangle_id, Some([29, 101, 233, 91]));
        let expected = document.render().unwrap();

        DraftStore::new(profile.path())
            .save(&identity, Some(&source_path), &document)
            .unwrap();
        drop(document);
        let mut restored = DraftStore::new(profile.path())
            .load(&identity, Some(&source_path))
            .unwrap()
            .unwrap();

        assert_eq!(
            fs::read(&source_path).unwrap(),
            b"source bytes must remain untouched"
        );
        assert_eq!(restored.render().unwrap(), expected);
        assert_eq!(
            restored.crop,
            Rect {
                x: 1.0,
                y: 1.0,
                width: 5.0,
                height: 3.0
            }
        );
        assert_eq!(restored.layers.len(), 2);
        assert!(restored.layers[0].original_pixels.is_some());
        assert_eq!(restored.layers[0].opacity, 173);
        assert_eq!(restored.layers[0].rotation_degrees, -17.0);
        assert!(restored.set_layer_opacity(image_id, 99));
        assert!(restored.undo());
        assert_eq!(restored.layers[0].opacity, 173);
        assert_eq!(
            fs::read(&source_path).unwrap(),
            b"source bytes must remain untouched"
        );
    }

    #[test]
    fn capture_and_imported_identities_do_not_share_drafts_for_the_same_path() {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("same.png");
        let capture = DraftIdentity::capture(path.clone());
        let imported = DraftIdentity::imported(path.clone());
        let mut capture_document =
            Document::new(RgbaImage::from_pixel(2, 3, Rgba([11, 23, 37, 255])));
        capture_document.set_background(Some([1, 2, 3, 255]));
        let mut imported_document =
            Document::new(RgbaImage::from_pixel(4, 1, Rgba([71, 83, 97, 255])));
        imported_document.set_background(Some([5, 6, 7, 255]));
        let store = DraftStore::new(profile.path());
        store
            .save(&capture, Some(&path), &capture_document)
            .unwrap();
        store
            .save(&imported, Some(&path), &imported_document)
            .unwrap();

        let restored_capture = store.load(&capture, Some(&path)).unwrap().unwrap();
        let restored_import = store.load(&imported, Some(&path)).unwrap().unwrap();
        assert_eq!(
            (
                restored_capture.canvas_width,
                restored_capture.canvas_height
            ),
            (2, 3)
        );
        assert_eq!(
            (restored_import.canvas_width, restored_import.canvas_height),
            (4, 1)
        );
        assert_eq!(restored_capture.background, Some([1, 2, 3, 255]));
        assert_eq!(restored_import.background, Some([5, 6, 7, 255]));
    }

    #[test]
    fn failed_snapshot_keeps_the_previous_generation_exact_after_restart() {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("capture.png");
        let identity = DraftIdentity::capture(path.clone());
        let store = DraftStore::new(profile.path());
        let mut previous = Document::new(RgbaImage::from_pixel(3, 2, Rgba([17, 43, 91, 255])));
        previous.set_background(Some([3, 7, 11, 255]));
        let expected = previous.render().unwrap();
        store.save(&identity, Some(&path), &previous).unwrap();
        let directory = store.root.join(identity.key());
        fs::rename(directory.join(CURRENT), directory.join(PREVIOUS)).unwrap();
        assert_eq!(
            store
                .load(&identity, Some(&path))
                .unwrap()
                .unwrap()
                .render()
                .unwrap(),
            expected,
            "restart during directory swap must fall back to the previous generation"
        );
        let missing_staging = directory.join(".missing-second-generation");
        assert!(publish_snapshot(&directory, &missing_staging).is_err());
        assert_eq!(
            store
                .load(&identity, Some(&path))
                .unwrap()
                .unwrap()
                .render()
                .unwrap(),
            expected,
            "a failed second publication must retain the recovered generation"
        );

        let mut changed = Document::new(RgbaImage::from_pixel(3, 2, Rgba([211, 71, 29, 255])));
        changed.set_background(Some([101, 103, 107, 255]));
        changed.add(
            Shape::Text {
                origin: Point { x: 1.0, y: 1.0 },
                value: "later failing asset".into(),
                font_size: 12.0,
                font_data: Arc::from(vec![0x5a; 2_048]),
                style: captures_image::TextStyleSettings::default(),
            },
            [255, 255, 255, 255],
            1.0,
        );
        let error = store
            .save_with_asset_limit(&identity, Some(&path), &changed, 1_024)
            .unwrap_err();
        assert!(error.contains("too large"));

        let restarted = DraftStore::new(profile.path())
            .load(&identity, Some(&path))
            .unwrap()
            .unwrap();
        assert_eq!(restarted.background, Some([3, 7, 11, 255]));
        assert_eq!(restarted.render().unwrap(), expected);
        assert!(restarted.layers.is_empty());
    }

    #[test]
    fn load_rejects_oversized_manifest_and_png_dimensions_before_decode() {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("capture.png");
        let identity = DraftIdentity::capture(path.clone());
        let store = DraftStore::new(profile.path());
        store
            .save(&identity, Some(&path), &Document::new(asymmetric_source()))
            .unwrap();
        let current = store.root.join(identity.key()).join(CURRENT);
        let original_manifest = fs::read(current.join(MANIFEST)).unwrap();
        fs::write(
            current.join(MANIFEST),
            vec![b' '; usize::try_from(MAX_MANIFEST_BYTES + 1).unwrap()],
        )
        .unwrap();
        assert!(store.load(&identity, Some(&path)).is_err());

        fs::write(current.join(MANIFEST), original_manifest).unwrap();
        let oversized = RgbaImage::from_pixel(MAX_IMAGE_DIMENSION + 1, 1, Rgba([211, 31, 73, 255]));
        oversized
            .save_with_format(current.join("assets/original.png"), ImageFormat::Png)
            .unwrap();
        let error = store
            .load(&identity, Some(&path))
            .err()
            .expect("oversized PNG must be rejected");
        assert!(error.contains("dimensions are invalid"));
    }

    #[test]
    fn new_capture_identity_only_restores_the_same_session_key_and_discard_is_scoped() {
        let profile = tempfile::tempdir().unwrap();
        let store = DraftStore::new(profile.path());
        let first = DraftIdentity::new_capture();
        let other = DraftIdentity::new_capture();
        let document = Document::new(asymmetric_source());
        store.save(&first, None, &document).unwrap();
        assert!(store.load(&first, None).unwrap().is_some());
        assert!(store.load(&other, None).unwrap().is_none());
        store.discard(&other).unwrap();
        assert!(store.load(&first, None).unwrap().is_some());
        store.discard(&first).unwrap();
        assert!(store.load(&first, None).unwrap().is_none());
    }

    #[test]
    fn deleting_a_matching_capture_retires_its_live_session_but_not_other_sessions() {
        let path = PathBuf::from(r"C:\Captures\Draft.PNG");
        let mut session = Some(DraftSession {
            identity: DraftIdentity::capture(path.clone()),
            persisted_key: (41, 2),
            observed_key: (41, 3),
            changed_at: Some(Instant::now()),
            in_flight: Some((41, 2)),
        });
        assert!(!retire_capture_session(
            &mut session,
            Path::new(r"C:\Captures\other.png")
        ));
        assert!(session.is_some());
        let matching_path = if cfg!(windows) {
            PathBuf::from(r"\\?\c:\captures\draft.png")
        } else {
            path
        };
        assert!(retire_capture_session(&mut session, &matching_path));
        assert!(session.is_none());

        // A delayed image-save completion can only transition an existing
        // session; deletion leaves none to rekey or flush on close.
        if let Some(session) = session.as_mut() {
            session.persisted_key = (41, 3);
        }
        assert!(session.is_none());
    }
}
