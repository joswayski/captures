//! Screenshot editor draft storage shared by desktop hosts.
//!
//! The caller supplies an isolated drafts directory and presentation URLs. The
//! version-1 document remains opaque JSON; this is not a native editor model.
//! Assets and the manifest are replaced atomically one file at a time, retaining
//! the shipping save order (not a transaction across the whole draft).

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use crate::Error;

const SCHEMA_VERSION: u16 = 1;
const MANIFEST_FILE: &str = "manifest.json";
const ASSETS_DIR: &str = "assets";
const FONTS_DIR: &str = "fonts";
const MAX_ASSETS: usize = 64;
const MAX_TOTAL_BYTES: usize = 80 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 256 * 1024 * 1024;

/// The shipping frontend matches this text to retry with all image bytes.
pub const ASSET_MISSING: &str =
    "a previously saved draft image asset is missing; resend the full draft";

/// `png: None` reuses an image already persisted by an earlier draft save.
#[derive(Debug, Deserialize)]
pub struct AssetInput {
    pub id: String,
    #[serde(default)]
    pub png: Option<Vec<u8>>,
}

#[derive(Debug, Deserialize)]
pub struct SaveRequest {
    pub artifact_id: String,
    pub document: serde_json::Value,
    pub assets: Vec<AssetInput>,
    pub updated_at_ms: u64,
}

/// Trusted font files supplied by a native host, never bytes in command JSON.
/// IDs identify immutable files; families map document keys to embedded names.
/// Storage validates sizes/IDs, not font parsing or redistribution permissions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontAssets {
    pub families: BTreeMap<String, String>,
    pub files: BTreeMap<String, Arc<[u8]>>,
    /// Full copyright/license notices retained with redistributed font bytes.
    pub notices: String,
}

impl FontAssets {
    pub fn validate(&self) -> Result<(), Error> {
        font_manifest(self).validate()?;
        let mut total = self.notices.len();
        for bytes in self.files.values() {
            total = total.saturating_add(bytes.len());
            if bytes.is_empty() || total > MAX_TOTAL_BYTES {
                return Err(Error::Invalid("Draft fonts are empty or too large.".into()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct FontManifest {
    families: BTreeMap<String, String>,
    assets: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    notices: String,
}

impl FontManifest {
    fn validate(&self) -> Result<(), Error> {
        if self.assets.is_empty()
            || self.notices.len() > 64 * 1024
            || self.assets.len() > MAX_ASSETS
            || self.families.is_empty()
            || self.families.len() > MAX_ASSETS
            || self.families.iter().any(|(key, name)| {
                key.trim().is_empty()
                    || name.trim().is_empty()
                    || key.len() > 256
                    || name.len() > 256
            })
            || self.assets.iter().collect::<HashSet<_>>().len() != self.assets.len()
        {
            return Err(Error::Invalid("Invalid draft font references.".into()));
        }
        for id in &self.assets {
            validate_component_id(id)?;
        }
        Ok(())
    }
}

fn font_manifest(fonts: &FontAssets) -> FontManifest {
    FontManifest {
        families: fonts.families.clone(),
        assets: fonts.files.keys().cloned().collect(),
        notices: fonts.notices.clone(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    schema_version: u16,
    artifact_id: String,
    updated_at_ms: u64,
    document: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fonts: Option<FontManifest>,
}

#[derive(Debug, Serialize)]
pub struct LoadedDraft {
    pub document: serde_json::Value,
    pub updated_at_ms: u64,
    #[serde(skip)]
    pub fonts: Option<FontAssets>,
}

/// Save the shipping layered document and incremental image assets. Validation
/// precedes filesystem mutation; removed assets are pruned before replacement.
pub fn save(drafts: &Path, request: SaveRequest) -> Result<(), Error> {
    save_with_fonts(drafts, request, None)
}

/// Native drafts retain exact font bytes beside images, within the same 80 MiB
/// aggregate budget. Font IDs cannot be rebound. Unreferenced font files are
/// pruned only after publishing the manifest; image save ordering is unchanged.
pub fn save_with_fonts(
    drafts: &Path,
    request: SaveRequest,
    fonts: Option<&FontAssets>,
) -> Result<(), Error> {
    validate_component_id(&request.artifact_id)?;
    if request.assets.len() > MAX_ASSETS {
        return Err(Error::Invalid(
            "this edit has too many image layers to keep as a draft".to_owned(),
        ));
    }
    let root = drafts.join(&request.artifact_id);
    let assets_dir = root.join(ASSETS_DIR);
    let fonts_dir = root.join(FONTS_DIR);
    let mut total_bytes = 0usize;
    if let Some(fonts) = fonts {
        fonts.validate()?;
        total_bytes += fonts.notices.len();
        for (id, bytes) in &fonts.files {
            total_bytes += bytes.len();
            let path = fonts_dir.join(format!("{id}.font"));
            if path.exists() && read_font(&path, MAX_TOTAL_BYTES)?.as_slice() != bytes.as_ref() {
                return Err(Error::Invalid(
                    "A draft font ID cannot change its bytes.".into(),
                ));
            }
        }
    }
    for asset in &request.assets {
        validate_component_id(&asset.id)?;
        let len = match &asset.png {
            Some(png) => {
                if png.is_empty() || png.len() > MAX_IMAGE_BYTES {
                    return Err(Error::Invalid(
                        "a draft image asset is empty or too large".to_owned(),
                    ));
                }
                png.len()
            }
            None => fs::metadata(asset_path(&assets_dir, &asset.id))
                .ok()
                .filter(std::fs::Metadata::is_file)
                .map(|metadata| usize::try_from(metadata.len()).unwrap_or(usize::MAX))
                .ok_or_else(|| Error::Invalid(ASSET_MISSING.to_owned()))?,
        };
        total_bytes = total_bytes.saturating_add(len);
        if total_bytes > MAX_TOTAL_BYTES {
            return Err(Error::Invalid(
                "unsaved edits are too large to keep as a draft; save a file first".to_owned(),
            ));
        }
    }

    if let Some(fonts) = fonts {
        fs::create_dir_all(&fonts_dir)?;
        for (id, bytes) in &fonts.files {
            let path = fonts_dir.join(format!("{id}.font"));
            if !path.exists() {
                let mut temporary = tempfile::NamedTempFile::new_in(&fonts_dir)?;
                temporary.write_all(bytes)?;
                temporary.as_file().sync_all()?;
                temporary
                    .persist_noclobber(path)
                    .map_err(|error| Error::Io(error.error))?;
            }
        }
    }
    fs::create_dir_all(&assets_dir)?;
    let keep: HashSet<PathBuf> = request
        .assets
        .iter()
        .map(|asset| asset_path(&assets_dir, &asset.id))
        .collect();
    for entry in fs::read_dir(&assets_dir)? {
        let path = entry?.path();
        if path.is_file() && !keep.contains(&path) {
            let _ = fs::remove_file(path);
        }
    }
    for asset in &request.assets {
        if let Some(png) = &asset.png {
            write_atomically(&asset_path(&assets_dir, &asset.id), png)?;
        }
    }

    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        artifact_id: request.artifact_id,
        updated_at_ms: request.updated_at_ms,
        document: request.document,
        fonts: fonts.map(font_manifest),
    };
    write_atomically(
        &root.join(MANIFEST_FILE),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    if let Ok(entries) = fs::read_dir(&fonts_dir) {
        let keep: HashSet<_> = fonts
            .into_iter()
            .flat_map(|fonts| fonts.files.keys())
            .map(|id| fonts_dir.join(format!("{id}.font")))
            .collect();
        for entry in entries.flatten() {
            if entry.path().is_file() && !keep.contains(&entry.path()) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

/// Restore a draft and map validated image references to host-owned resources.
/// Unknown schemas and missing/invalid image references are discarded, as in
/// the shipping editor. Font, JSON and mismatched-folder errors preserve the draft.
pub fn load(
    drafts: &Path,
    artifact_id: &str,
    mut asset_url: impl FnMut(&str, &str) -> String,
) -> Result<Option<LoadedDraft>, Error> {
    validate_component_id(artifact_id)?;
    let root = drafts.join(artifact_id);
    let manifest_path = root.join(MANIFEST_FILE);
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let mut manifest: Manifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    if manifest.schema_version != SCHEMA_VERSION {
        let _ = discard(drafts, artifact_id);
        return Ok(None);
    }
    if manifest.artifact_id != artifact_id {
        return Err(Error::Invalid(
            "screenshot editor draft does not match its folder".to_owned(),
        ));
    }
    // Unlike shipping image recovery, font errors preserve the draft for repair.
    let fonts = manifest
        .fonts
        .map(|fonts| {
            fonts.validate()?;
            let mut remaining = MAX_TOTAL_BYTES - fonts.notices.len();
            let mut files = BTreeMap::new();
            for id in fonts.assets {
                let bytes = read_font(&root.join(FONTS_DIR).join(format!("{id}.font")), remaining)?;
                remaining -= bytes.len();
                files.insert(id, Arc::from(bytes));
            }
            Ok::<_, Error>(FontAssets {
                families: fonts.families,
                files,
                notices: fonts.notices,
            })
        })
        .transpose()?;
    if !rewrite_asset_urls(
        &mut manifest.document,
        &root.join(ASSETS_DIR),
        artifact_id,
        &mut asset_url,
    ) {
        let _ = discard(drafts, artifact_id);
        return Ok(None);
    }
    Ok(Some(LoadedDraft {
        document: manifest.document,
        updated_at_ms: manifest.updated_at_ms,
        fonts,
    }))
}

fn read_font(path: &Path, budget: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(budget as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() > budget {
        return Err(Error::Invalid("Draft fonts are empty or too large.".into()));
    }
    Ok(bytes)
}

/// Delete only the selected draft, leaving capture history and exports alone.
pub fn discard(drafts: &Path, artifact_id: &str) -> Result<(), Error> {
    validate_component_id(artifact_id)?;
    let root = drafts.join(artifact_id);
    if !root.exists() {
        return Ok(());
    }
    fs::remove_dir_all(root)?;
    Ok(())
}

pub fn read_asset(drafts: &Path, artifact_id: &str, asset_id: &str) -> Result<Vec<u8>, Error> {
    validate_component_id(artifact_id)?;
    validate_component_id(asset_id)?;
    Ok(fs::read(asset_path(
        &drafts.join(artifact_id).join(ASSETS_DIR),
        asset_id,
    ))?)
}

fn asset_path(assets_dir: &Path, asset_id: &str) -> PathBuf {
    assets_dir.join(format!("{asset_id}.png"))
}

fn validate_component_id(id: &str) -> Result<(), Error> {
    if id.is_empty()
        || id.len() > 80
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(Error::Invalid(
            "invalid screenshot editor draft id".to_owned(),
        ));
    }
    Ok(())
}

fn rewrite_asset_urls(
    document: &mut serde_json::Value,
    assets_dir: &Path,
    artifact_id: &str,
    asset_url: &mut impl FnMut(&str, &str) -> String,
) -> bool {
    let Some(elements) = document
        .get_mut("elements")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return true;
    };
    for element in elements {
        if element.get("kind").and_then(serde_json::Value::as_str) != Some("image") {
            continue;
        }
        for field in ["src", "originalSrc"] {
            let Some(value) = element.get(field).and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(asset_id) = value.strip_prefix("draft-asset:") else {
                continue;
            };
            if validate_component_id(asset_id).is_err()
                || !asset_path(assets_dir, asset_id).is_file()
            {
                return false;
            }
            element[field] = serde_json::Value::String(asset_url(artifact_id, asset_id));
        }
    }
    true
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path.parent().ok_or_else(|| {
        Error::Invalid("the edited screenshot path has no destination folder".to_owned())
    })?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| Error::Io(error.error))?;
    Ok(())
}
