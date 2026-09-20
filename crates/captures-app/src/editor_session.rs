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
    editor::{Document, DocumentHistory, Element, LayerEdit, OptionalNullable, Rect},
    editor_render::{MAX_RENDER_DIMENSION, MAX_RENDER_PIXELS, render},
};

const ASSET_PREFIX: &str = "draft-asset:";
const MAX_METADATA_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct OpenRequest {
    pub history_root: PathBuf,
    pub drafts_root: PathBuf,
    pub artifact_id: String,
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
