//! Explicit offline import and snapshotting of isolated native development profiles.
//! Sources remain untouched. Hosts never invoke this or discover installed paths.
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use captures_media::CancelToken;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ROOTS: [(&str, &str); 3] = [
    ("capture-history", "history"),
    ("screenshot-editor-drafts", "editor-drafts"),
    ("recording-recovery", "recording-recovery"),
];
const DEVELOPMENT_MARKER: &str = ".captures-native-development-profile.json";
const NATIVE_ROOTS: [&str; 5] = [
    "settings.json",
    "history",
    "editor-drafts",
    "recording-recovery",
    DEVELOPMENT_MARKER,
];
const MAX_ENTRIES: usize = 100_000;
const MAX_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Settings(#[from] captures_settings::SettingsError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("Profile import cancelled")]
    Cancelled,
}

#[derive(Debug, Serialize)]
pub struct ImportReport {
    pub copied_files: usize,
    pub copied_bytes: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct TreeDigest {
    sha256: String,
    files: usize,
    bytes: u64,
}

#[derive(Default)]
struct Budget {
    entries: usize,
    bytes: u64,
}

struct ImportLock(File);
impl Drop for ImportLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevelopmentMarker {
    schema_version: u8,
    identity: String,
    root: PathBuf,
}

// Only new empty/imported profiles are enrolled. Bind the marker to the FINAL
// canonical root, not an import's scratch directory. This is cooperative safety,
// not authentication against a malicious same-user writer.
pub(crate) fn mark_development_profile(root: &Path, destination: &Path) -> Result<(), Error> {
    let mut marker = File::options()
        .write(true)
        .create_new(true)
        .open(root.join(DEVELOPMENT_MARKER))?;
    serde_json::to_writer_pretty(
        &mut marker,
        &DevelopmentMarker {
            schema_version: 1,
            identity: crate::updater::DEVELOPMENT_IDENTITY.into(),
            root: destination.to_owned(),
        },
    )?;
    marker.sync_all()?;
    Ok(())
}

/// Validate an explicit, previously created development profile without enrolling
/// it, discovering installed paths, or migrating/writing its settings. Copied or
/// moved markers do not authorize a different canonical root. Parents must remain
/// trusted; this marker is not a security boundary against its owner.
pub fn validate_development_profile(root: &Path) -> Result<PathBuf, Error> {
    if !root.is_absolute() {
        return Err(invalid(
            "Select an explicit absolute development profile path",
        ));
    }
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid(
            "Select a real absolute development profile directory",
        ));
    }
    let root = root.canonicalize()?;
    let marker_path = root.join(DEVELOPMENT_MARKER);
    let metadata = match fs::symlink_metadata(&marker_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(invalid("Profile is not enrolled as a development profile"));
        }
        metadata => metadata?,
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("Development profile marker must be a regular file"));
    }
    let marker: DevelopmentMarker = serde_json::from_value(json(&marker_path)?)?;
    if marker.schema_version != 1
        || marker.identity != crate::updater::DEVELOPMENT_IDENTITY
        || marker.root != root
    {
        return Err(invalid("Profile is not enrolled at this development root"));
    }
    let settings_path = root.join("settings.json");
    let metadata = fs::symlink_metadata(&settings_path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("Development settings must be a regular file"));
    }
    // captures_settings::load can WRITE migrations, so do not use it here.
    let settings: captures_settings::AppSettings = serde_json::from_value(json(&settings_path)?)?;
    if settings.settings_schema_version > captures_settings::CURRENT_SETTINGS_SCHEMA_VERSION {
        return Err(invalid(
            "Development settings use a newer unsupported schema",
        ));
    }
    for name in &NATIVE_ROOTS[1..4] {
        match fs::symlink_metadata(root.join(name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(invalid("Development data roots must be real directories")),
        }
    }
    Ok(root)
}

/// A validated working profile and a retained pre-update data snapshot.
/// Create BEFORE package activation, with every app/data writer stopped and
/// excluded throughout. The bounded private sibling snapshot includes settings,
/// History (including its local diagnostics), editor drafts, recording recovery
/// and the development marker, not external exports, OS credentials, profile-root
/// logs or previous snapshots. No secret store is queried or copied.
/// It survives success/failure/cancellation after preparation. There is no
/// automatic profile copyback or cross-platform power-loss durability guarantee.
pub struct PreparedDevelopmentProfile {
    root: PathBuf,
    snapshot: PathBuf,
    digests: Vec<Option<TreeDigest>>,
}

impl PreparedDevelopmentProfile {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn snapshot(&self) -> &Path {
        &self.snapshot
    }

    /// Recheck original AND snapshot bytes immediately before activation/launch.
    /// Exposed paths are mutable; a prepared snapshot is not an immutable handle.
    pub fn verify(&self, cancel: &CancelToken) -> Result<(), Error> {
        check_cancel(cancel)?;
        validate_development_profile(&self.root)?;
        let metadata = fs::symlink_metadata(&self.snapshot)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid("Pre-update snapshot must be a real directory"));
        }
        for root in [&self.root, &self.snapshot] {
            let mut budget = Budget::default();
            for (name, expected) in NATIVE_ROOTS.iter().zip(&self.digests) {
                if tree(&root.join(name), None, cancel, &mut budget)? != *expected {
                    return Err(invalid(
                        "Development profile or pre-update snapshot changed",
                    ));
                }
            }
        }
        Ok(())
    }
}

pub fn prepare_development_profile(
    root: &Path,
    cancel: &CancelToken,
) -> Result<PreparedDevelopmentProfile, Error> {
    check_cancel(cancel)?;
    let root = validate_development_profile(root)?;
    let stage = tempfile::Builder::new()
        .prefix(".captures-native-pre-update-")
        .tempdir_in(
            root.parent()
                .ok_or_else(|| invalid("Invalid profile parent"))?,
        )?;
    let mut budget = Budget::default();
    let mut digests = Vec::new();
    for name in NATIVE_ROOTS {
        digests.push(tree(
            &root.join(name),
            Some(&stage.path().join(name)),
            cancel,
            &mut budget,
        )?);
    }
    let prepared = PreparedDevelopmentProfile {
        root,
        snapshot: stage.path().to_owned(),
        digests,
    };
    prepared.verify(cancel)?;
    let mut receipt = File::create(stage.path().join("snapshot-receipt.json"))?;
    serde_json::to_writer_pretty(
        &mut receipt,
        &serde_json::json!({"schema_version":1, "source_root":prepared.root,
            "roots":NATIVE_ROOTS, "source_trees":prepared.digests}),
    )?;
    receipt.sync_all()?;
    drop(receipt);
    check_cancel(cancel)?;
    let _ = stage.keep();
    Ok(prepared)
}

/// Copy explicitly selected SHIPPING settings/data into a new, isolated native
/// development root with `settings.json`, `history`, `editor-drafts` and
/// `recording-recovery`. All app processes/writers must be stopped and the
/// destination parent must be trusted throughout. This does not detect them.
///
/// Retain original bytes/empty directories in `source-snapshot`; schema migration
/// and path/permission resets affect only the working copy. No external saved
/// files, accounts, OS credentials, login registrations or diagnostics are copied.
/// Reference-only recordings and unfinished publication intents require explicit
/// reconciliation before import. Failure/cancellation leave no destination.
/// A forced termination can leave private sibling scratch, never a partial
/// published profile. This is not cross-platform power-loss durability.
pub fn import_shipping_profile(
    settings_file: &Path,
    data_root: &Path,
    destination: &Path,
    cancel: &CancelToken,
) -> Result<ImportReport, Error> {
    import_with(settings_file, data_root, destination, cancel, || Ok(()))
}

fn import_with(
    settings_file: &Path,
    data_root: &Path,
    destination: &Path,
    cancel: &CancelToken,
    before_publish: impl FnOnce() -> Result<(), Error>,
) -> Result<ImportReport, Error> {
    check_cancel(cancel)?;
    if !settings_file.is_absolute() || !data_root.is_absolute() || !destination.is_absolute() {
        return Err(invalid("All profile paths must be explicit absolute paths"));
    }
    let settings_metadata = fs::symlink_metadata(settings_file)?;
    let data_metadata = fs::symlink_metadata(data_root)?;
    if !settings_metadata.is_file()
        || settings_metadata.file_type().is_symlink()
        || settings_metadata.len() > MAX_JSON_BYTES
        || !data_metadata.is_dir()
        || data_metadata.file_type().is_symlink()
    {
        return Err(invalid("Select real source settings/data, not links"));
    }
    let settings_file = settings_file.canonicalize()?;
    let data_root = data_root.canonicalize()?;
    let name = destination
        .file_name()
        .filter(|_| {
            matches!(
                destination.components().next_back(),
                Some(Component::Normal(_))
            )
        })
        .ok_or_else(|| invalid("Select a new profile directory"))?;
    let parent = destination.parent().unwrap().canonicalize()?;
    let destination = parent.join(name);
    if destination.to_str().is_none() {
        return Err(invalid("The native profile path must be valid Unicode"));
    }
    if destination.starts_with(&data_root) || settings_file.starts_with(&destination) {
        return Err(invalid("The new profile must be outside the source data"));
    }
    let lock_path = parent.join(format!(
        ".captures-native-profile-{:x}.lock",
        Sha256::digest(name.as_encoded_bytes())
    ));
    if let Ok(metadata) = fs::symlink_metadata(&lock_path)
        && (!metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(invalid("Profile import lock is not a regular file"));
    }
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    lock.try_lock()
        .map_err(|_| invalid("Another profile import is active"))?;
    let _lock = ImportLock(lock);
    require_absent(&destination)?;
    let stage = tempfile::Builder::new()
        .prefix(".captures-native-profile-")
        .tempdir_in(&parent)?;
    let backup = stage.path().join("source-snapshot");
    fs::create_dir(&backup)?;
    let sources: Vec<_> = std::iter::once((settings_file, "settings.json"))
        .chain(
            ROOTS
                .iter()
                .map(|(source, _)| (data_root.join(source), *source)),
        )
        .collect();
    let mut budget = Budget::default();
    let mut digests = Vec::new();
    for (source, name) in &sources {
        digests.push(tree(source, Some(&backup.join(name)), cancel, &mut budget)?);
    }
    let mut copied = Budget::default();
    for ((_, name), expected) in sources.iter().zip(&digests) {
        let target = ROOTS
            .iter()
            .find(|(source, _)| source == name)
            .map_or("settings.json", |(_, target)| *target);
        if tree(
            &backup.join(name),
            Some(&stage.path().join(target)),
            cancel,
            &mut copied,
        )? != *expected
        {
            return Err(invalid("Source snapshot changed during preparation"));
        }
    }
    prepare_working_copy(stage.path(), &destination, cancel)?;
    mark_development_profile(stage.path(), &destination)?;
    before_publish()?;
    let mut checked = Budget::default();
    for ((source, _), expected) in sources.iter().zip(&digests) {
        if tree(source, None, cancel, &mut checked)? != *expected {
            return Err(invalid(
                "Source changed during import; no profile was published",
            ));
        }
    }
    let report = ImportReport {
        copied_files: digests.iter().flatten().map(|digest| digest.files).sum(),
        copied_bytes: digests.iter().flatten().map(|digest| digest.bytes).sum(),
    };
    let mut receipt = File::create(stage.path().join("import-receipt.json"))?;
    serde_json::to_writer_pretty(
        &mut receipt,
        &serde_json::json!({
            "schema_version": 1, "source_trees": digests, "source_files": report.copied_files,
            "source_bytes": report.copied_bytes, "installed_source_modified": false
        }),
    )?;
    receipt.sync_all()?;
    drop(receipt);
    check_cancel(cancel)?;
    require_absent(&destination)?;
    fs::rename(stage.path(), &destination)?;
    Ok(report)
}

fn prepare_working_copy(
    root: &Path,
    destination: &Path,
    cancel: &CancelToken,
) -> Result<(), Error> {
    let settings_path = root.join("settings.json");
    if fs::metadata(&settings_path)?.len() > MAX_JSON_BYTES {
        return Err(invalid("Profile JSON is too large"));
    }
    let mut settings = captures_settings::load(&settings_path)?;
    settings.output_directory = destination.join("exports").to_string_lossy().into_owned();
    settings.launch_at_login = false;
    settings.onboarding_completed = false;
    settings.last_screen_permission_request_id = None;
    settings.pending_capture_after_restart = None;
    captures_settings::validate(&settings)?;
    captures_settings::write_atomic(&settings_path, &settings)?;
    fs::create_dir(root.join("exports"))?;
    for (source, working) in ROOTS {
        check_cancel(cancel)?;
        let directory = root.join(working);
        if !directory.exists() {
            continue;
        }
        for item in fs::read_dir(&directory)? {
            check_cancel(cancel)?;
            let item = item?;
            let id = item.file_name().to_string_lossy().into_owned();
            if id.starts_with('.') {
                continue;
            }
            if !item.file_type()?.is_dir() || uuid::Uuid::parse_str(&id).is_err() {
                return Err(invalid("Source contains an unrecognized artifact or draft"));
            }
            let path = item.path();
            match source {
                "capture-history" => {
                    let metadata = path.join(captures_history::HISTORY_METADATA_FILE);
                    let mut value = json(&metadata)?;
                    let entry: captures_history::HistoryEntry =
                        serde_json::from_value(value.clone())?;
                    if entry.id != id
                        || chrono::DateTime::parse_from_rfc3339(&entry.created_at).is_err()
                    {
                        return Err(invalid("History metadata does not match its artifact"));
                    }
                    if !path.join(captures_history::HISTORY_PREVIEW_FILE).is_file()
                        || (entry.kind == captures_history::ArtifactKind::Screenshot
                            && (entry.mode.is_none()
                                || !path.join(captures_history::HISTORY_IMAGE_FILE).is_file()))
                        || (entry.kind.is_recording()
                            && (entry.mime_type.is_none()
                                || entry.duration_ms.is_none()
                                || entry.target.is_none()))
                    {
                        return Err(invalid(
                            "History media is incomplete; reconcile it before import",
                        ));
                    }
                    if entry.kind.is_recording()
                        && captures_history::find_recording_media(&path).is_none()
                    {
                        return Err(invalid(
                            "Reference-only recordings must be reconciled before import; external exports were not read",
                        ));
                    }
                    // Never let development Replace original/Trash act on shipping exports.
                    value["saved_path"] = serde_json::Value::Null;
                    fs::write(metadata, serde_json::to_vec_pretty(&value)?)?;
                }
                "screenshot-editor-drafts" => {
                    let manifest = json(&path.join("manifest.json"))?;
                    if manifest["schema_version"].as_u64() != Some(1)
                        || manifest["artifact_id"].as_str() != Some(id.as_str())
                    {
                        return Err(invalid("Unsupported or mismatched screenshot draft"));
                    }
                    let draft = captures_history::editor_draft::load(
                        &directory,
                        &id,
                        |_, asset| format!("draft-asset:{asset}"),
                    )
                    .map_err(|error| invalid(&error.to_string()))?
                    .ok_or_else(|| {
                        invalid(
                            "Screenshot draft schema/assets require reconciliation before import",
                        )
                    })?;
                    let _: crate::editor::Document = serde_json::from_value(draft.document)?;
                }
                "recording-recovery" => {
                    if path.join("publication-intent-v1.json").exists() {
                        return Err(invalid(
                            "Finish recording publication recovery before importing this profile",
                        ));
                    }
                    let metadata = path.join("manifest.json");
                    let mut value = json(&metadata)?;
                    let manifest: captures_recording::RecordingDraftManifest =
                        serde_json::from_value(value.clone())?;
                    if manifest.schema_version != 1 || manifest.session_id != id {
                        return Err(invalid("Unsupported or mismatched recording draft"));
                    }
                    for segment in &manifest.segments {
                        for relative in [
                            Some(&segment.relative_path),
                            segment.system_audio_relative_path.as_ref(),
                            segment.microphone_relative_path.as_ref(),
                        ]
                        .into_iter()
                        .flatten()
                        {
                            if relative.is_empty()
                                || relative.contains('\\')
                                || Path::new(relative)
                                    .components()
                                    .any(|part| !matches!(part, Component::Normal(_)))
                            {
                                return Err(invalid(
                                    "Recording draft media must stay inside its bundle",
                                ));
                            }
                            if !path.join(relative).is_file() {
                                return Err(invalid(
                                    "Recording draft media is missing; reconcile it before import",
                                ));
                            }
                        }
                    }
                    value["final_path"] = serde_json::Value::Null;
                    fs::write(metadata, serde_json::to_vec_pretty(&value)?)?;
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(())
}

fn json(path: &Path) -> Result<serde_json::Value, Error> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(invalid("Profile JSON is too large"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

// Bounded streaming copy/digest over regular files and real directories only.
// Frame relative paths/types/lengths, including empty directories; ignore times.
fn tree(
    source: &Path,
    destination: Option<&Path>,
    cancel: &CancelToken,
    budget: &mut Budget,
) -> Result<Option<TreeDigest>, Error> {
    match fs::symlink_metadata(source) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let mut hash = Sha256::new();
    let mut pending = vec![source.to_owned()];
    let mut files = 0;
    let mut bytes = 0;
    let mut buffer = [0; 64 * 1024];
    while let Some(path) = pending.pop() {
        check_cancel(cancel)?;
        budget.entries += 1;
        if budget.entries > MAX_ENTRIES {
            return Err(invalid("Profile has too many entries"));
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
            return Err(invalid("Profile contains a link or special file"));
        }
        let relative = path.strip_prefix(source).unwrap();
        let name = relative.as_os_str().as_encoded_bytes();
        if name.len() > 4096 {
            return Err(invalid("Profile path is too long"));
        }
        hash.update([u8::from(metadata.is_file())]);
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name);
        let target = destination.map(|destination| {
            if relative.as_os_str().is_empty() {
                destination.to_owned()
            } else {
                destination.join(relative)
            }
        });
        if metadata.is_dir() {
            if let Some(target) = target {
                fs::create_dir(target)?;
            }
            let mut children = Vec::new();
            for entry in fs::read_dir(path)? {
                check_cancel(cancel)?;
                if budget.entries + pending.len() + children.len() >= MAX_ENTRIES {
                    return Err(invalid("Profile has too many entries"));
                }
                children.push(entry?.path());
            }
            children.sort();
            pending.extend(children.into_iter().rev());
        } else {
            budget.bytes = budget
                .bytes
                .checked_add(metadata.len())
                .ok_or_else(|| invalid("Profile is too large"))?;
            if budget.bytes > MAX_BYTES {
                return Err(invalid("Profile exceeds the 64 GiB source limit"));
            }
            files += 1;
            bytes += metadata.len();
            hash.update(metadata.len().to_le_bytes());
            let mut input = File::open(path)?.take(metadata.len() + 1);
            let mut output = target
                .map(|target| File::options().write(true).create_new(true).open(target))
                .transpose()?;
            let mut read = 0;
            loop {
                check_cancel(cancel)?;
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                read += count as u64;
                hash.update(&buffer[..count]);
                if let Some(output) = output.as_mut() {
                    output.write_all(&buffer[..count])?;
                }
            }
            if read != metadata.len() {
                return Err(invalid("Profile file changed during import"));
            }
            if let Some(output) = output {
                output.sync_all()?;
            }
        }
    }
    Ok(Some(TreeDigest {
        sha256: format!("{:x}", hash.finalize()),
        files,
        bytes,
    }))
}

fn require_absent(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(invalid(
            "The destination already exists; nothing was overwritten",
        )),
    }
}
fn check_cancel(cancel: &CancelToken) -> Result<(), Error> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::editor_session::{EditorSession, OpenRequest, Request};
    use captures_capture::CaptureMode;
    use image::{Rgba, RgbaImage};
    use serde_json::json;

    struct Fixture {
        root: tempfile::TempDir,
        settings: PathBuf,
        data: PathBuf,
        destination: PathBuf,
        capture_id: String,
        recovery_id: String,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let settings = root.path().join("shipping settings é.json");
            let data = root.path().join("shipping data");
            let destination = root.path().join("new native profile é");
            let mut preferences = captures_settings::AppSettings {
                settings_schema_version: 0,
                appearance: captures_settings::Appearance::Dark,
                launch_at_login: true,
                onboarding_completed: true,
                last_screen_permission_request_id: Some("shipping-identity".into()),
                pending_capture_after_restart: Some(CaptureMode::Window),
                ..Default::default()
            };
            preferences.recording.video_fps = 30;
            preferences.recording.gif_fps = 27;
            preferences.recording.gif_max_width = 704;
            captures_settings::write_atomic(&settings, &preferences).unwrap();
            let source =
                RgbaImage::from_fn(7, 3, |x, y| Rgba([x as u8 * 31, y as u8 * 71, 19, 255]));
            let capture = crate::persist_screenshot(
                &data.join("capture-history"),
                &source,
                CaptureMode::Region,
            )
            .unwrap();
            let capture_id = capture.entry.id;
            let mut editor = EditorSession::open(OpenRequest {
                history_root: data.join("capture-history"),
                drafts_root: data.join("screenshot-editor-drafts"),
                artifact_id: capture_id.clone(),
            })
            .unwrap();
            editor
                .execute(Request::Crop {
                    rect: crate::editor::Rect {
                        x: 2.,
                        y: 1.,
                        width: 4.,
                        height: 2.,
                    },
                })
                .unwrap();
            editor
                .execute(Request::SaveDraft {
                    updated_at_ms: 123_456,
                })
                .unwrap();
            fs::create_dir_all(data.join("capture-history/.retained/empty é")).unwrap();
            fs::write(
                data.join("account-session.json"),
                b"fake account data must not be copied",
            )
            .unwrap();
            let external = root.path().join("shipping export.png");
            fs::write(&external, b"do not overwrite or trash this external export").unwrap();
            let metadata = data
                .join("capture-history")
                .join(&capture_id)
                .join("metadata.json");
            let mut value = super::json(&metadata).unwrap();
            value["saved_path"] = external.to_string_lossy().into_owned().into();
            value["future_owned"] = json!({"asymmetric": [31, 7, 19]});
            fs::write(metadata, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
            let recovery_id = uuid::Uuid::new_v4().to_string();
            let options = serde_json::from_value(json!({"kind":"video", "target":{"type":"display","display_id":"fixture-17"},
                "frames_per_second":60, "max_resolution":"original", "countdown_seconds":0,"show_cursor":true})).unwrap();
            let mut manifest = captures_recording::RecordingDraftManifest::new(
                recovery_id.clone(),
                options,
                987_654,
            );
            manifest.state = captures_recording::RecordingState::Paused;
            manifest.final_path = Some(external.to_string_lossy().into_owned());
            manifest.segments.push(
                serde_json::from_value(json!({
                    "index": 3, "relative_path": "segment-003.mp4", "started_at_ms": 733,
                    "duration_ms": 419, "width": 17, "height": 9, "size_bytes": 28,
                    "complete": true, "dropped_frames": 7
                }))
                .unwrap(),
            );
            captures_recording::DraftStore::new(data.join("recording-recovery"))
                .create(&manifest)
                .unwrap();
            fs::write(
                data.join("recording-recovery")
                    .join(&recovery_id)
                    .join("segment-003.mp4"),
                b"asymmetric recovery source\0\xff",
            )
            .unwrap();
            Self {
                root,
                settings,
                data,
                destination,
                capture_id,
                recovery_id,
            }
        }

        fn import(&self) -> Result<ImportReport, Error> {
            import_shipping_profile(
                &self.settings,
                &self.data,
                &self.destination,
                &CancelToken::default(),
            )
        }

        fn no_partial_profile(&self) {
            assert!(!self.destination.exists());
            assert!(!fs::read_dir(self.root.path()).unwrap().any(|entry| {
                let entry = entry.unwrap();
                entry.file_type().unwrap().is_dir()
                    && entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".captures-native-profile-")
            }));
        }
    }

    #[test]
    fn import_preserves_source_snapshot_pixels_and_drafts_but_isolates_permissions_and_exports() {
        let fixture = Fixture::new();
        let settings_before = fs::read(&fixture.settings).unwrap();
        let relative_metadata = PathBuf::from("capture-history")
            .join(&fixture.capture_id)
            .join("metadata.json");
        let metadata_before = fs::read(fixture.data.join(&relative_metadata)).unwrap();
        let manifest_before = fs::read(
            fixture
                .data
                .join("screenshot-editor-drafts")
                .join(&fixture.capture_id)
                .join("manifest.json"),
        )
        .unwrap();
        let report = fixture.import().unwrap();
        assert_eq!(report.copied_files, 8);
        assert!(report.copied_bytes > settings_before.len() as u64);
        assert_eq!(fs::read(&fixture.settings).unwrap(), settings_before);
        assert_eq!(
            fs::read(fixture.destination.join("source-snapshot/settings.json")).unwrap(),
            settings_before
        );
        assert_eq!(
            fs::read(fixture.data.join(&relative_metadata)).unwrap(),
            metadata_before
        );
        assert_eq!(
            fs::read(
                fixture
                    .destination
                    .join("source-snapshot")
                    .join(&relative_metadata)
            )
            .unwrap(),
            metadata_before
        );
        assert_eq!(
            fs::read(
                fixture
                    .destination
                    .join("source-snapshot/screenshot-editor-drafts")
                    .join(&fixture.capture_id)
                    .join("manifest.json")
            )
            .unwrap(),
            manifest_before
        );
        assert!(
            fixture
                .destination
                .join("source-snapshot/capture-history/.retained/empty é")
                .is_dir()
        );
        assert!(
            !fixture
                .destination
                .join("source-snapshot/account-session.json")
                .exists()
        );
        assert!(!fixture.destination.join("account-session.json").exists());
        let settings = captures_settings::load(&fixture.destination.join("settings.json")).unwrap();
        assert_eq!(
            settings.settings_schema_version,
            captures_settings::CURRENT_SETTINGS_SCHEMA_VERSION
        );
        assert_eq!(settings.appearance, captures_settings::Appearance::Dark);
        assert_eq!(
            settings.recording.video_fps, 60,
            "migration occurs only in the working copy"
        );
        assert_eq!(settings.recording.gif_fps, 27);
        assert_eq!(settings.recording.gif_max_width, 704);
        assert!(!settings.launch_at_login);
        assert!(!settings.onboarding_completed);
        assert!(settings.last_screen_permission_request_id.is_none());
        assert!(settings.pending_capture_after_restart.is_none());
        assert_eq!(
            PathBuf::from(settings.output_directory),
            fixture.destination.canonicalize().unwrap().join("exports")
        );
        let metadata = super::json(
            &fixture
                .destination
                .join("history")
                .join(&fixture.capture_id)
                .join("metadata.json"),
        )
        .unwrap();
        assert!(metadata["saved_path"].is_null());
        assert_eq!(metadata["future_owned"], json!({"asymmetric": [31, 7, 19]}));
        let editor = EditorSession::open(OpenRequest {
            history_root: fixture.destination.join("history"),
            drafts_root: fixture.destination.join("editor-drafts"),
            artifact_id: fixture.capture_id.clone(),
        })
        .unwrap();
        assert_eq!(editor.pixels().dimensions(), (4, 2));
        for y in 0..2 {
            for x in 0..4 {
                assert_eq!(
                    editor.pixels().get_pixel(x, y).0,
                    [(x + 2) as u8 * 31, (y + 1) as u8 * 71, 19, 255]
                );
            }
        }
        let recovery = fixture
            .destination
            .join("recording-recovery")
            .join(&fixture.recovery_id);
        assert_eq!(
            fs::read(recovery.join("segment-003.mp4")).unwrap(),
            b"asymmetric recovery source\0\xff"
        );
        let copied_manifest = super::json(&recovery.join("manifest.json")).unwrap();
        assert!(copied_manifest["final_path"].is_null());
        assert_eq!(copied_manifest["segments"][0]["dropped_frames"], 7);
        let original_manifest = super::json(
            &fixture
                .destination
                .join("source-snapshot/recording-recovery")
                .join(&fixture.recovery_id)
                .join("manifest.json"),
        )
        .unwrap();
        assert_eq!(
            original_manifest["final_path"],
            fixture
                .root
                .path()
                .join("shipping export.png")
                .to_string_lossy()
                .as_ref()
        );
        assert_eq!(original_manifest["segments"], copied_manifest["segments"]);
        assert_eq!(
            fs::read(fixture.root.path().join("shipping export.png")).unwrap(),
            b"do not overwrite or trash this external export"
        );
    }

    #[test]
    fn newer_schemas_missing_media_and_publication_intents_preserve_source_without_publication() {
        for scenario in [
            "settings",
            "draft",
            "publication",
            "reference",
            "missing",
            "missing-mode",
            "missing-segment",
            "escaped-segment",
        ] {
            let fixture = Fixture::new();
            let path = match scenario {
                "settings" => fixture.settings.clone(),
                "draft" => fixture
                    .data
                    .join("screenshot-editor-drafts")
                    .join(&fixture.capture_id)
                    .join("manifest.json"),
                "publication" => fixture
                    .data
                    .join("recording-recovery")
                    .join(&fixture.recovery_id)
                    .join("publication-intent-v1.json"),
                "missing-segment" | "escaped-segment" => fixture
                    .data
                    .join("recording-recovery")
                    .join(&fixture.recovery_id)
                    .join("manifest.json"),
                _ => fixture
                    .data
                    .join("capture-history")
                    .join(&fixture.capture_id)
                    .join("metadata.json"),
            };
            let mut value = if scenario == "publication" {
                json!({"source_identity":"old-root"})
            } else {
                super::json(&path).unwrap()
            };
            match scenario {
                "settings" => value["settings_schema_version"] = 99.into(),
                "draft" => value["schema_version"] = 99.into(),
                "missing-mode" => value["mode"] = serde_json::Value::Null,
                "missing-segment" => value["segments"][0]["relative_path"] = "absent.mp4".into(),
                "escaped-segment" => {
                    value["segments"][0]["relative_path"] = "../shipping export.png".into()
                }
                "reference" => {
                    value["kind"] = "video".into();
                    value["mime_type"] = "video/mp4".into();
                    value["duration_ms"] = 2_000.into();
                    value["target"] = json!({"type":"display","display_id":"17"});
                }
                "missing" => {
                    fs::remove_file(
                        fixture
                            .data
                            .join("capture-history")
                            .join(&fixture.capture_id)
                            .join("capture.png"),
                    )
                    .unwrap();
                }
                _ => {}
            }
            let before = serde_json::to_vec_pretty(&value).unwrap();
            fs::write(&path, &before).unwrap();
            assert!(fixture.import().is_err(), "{scenario}");
            assert_eq!(
                fs::read(path).unwrap(),
                before,
                "{scenario} source unchanged"
            );
            fixture.no_partial_profile();
        }
    }

    #[test]
    fn cancel_source_changes_concurrent_import_and_late_destination_collision_never_publish_partial_data()
     {
        for scenario in ["cancel", "changed", "added", "collision", "lock"] {
            let fixture = Fixture::new();
            let cancel = CancelToken::default();
            let result = import_with(
                &fixture.settings,
                &fixture.data,
                &fixture.destination,
                &cancel,
                || {
                    match scenario {
                        "cancel" => cancel.cancel(),
                        "changed" => {
                            fs::write(&fixture.settings, b"external writer changed source").unwrap()
                        }
                        "added" => {
                            fs::write(fixture.data.join("capture-history/.new-file"), b"new bytes")
                                .unwrap()
                        }
                        "collision" => {
                            fs::create_dir(&fixture.destination).unwrap();
                            fs::write(
                                fixture.destination.join("sentinel"),
                                b"preserve a concurrent destination",
                            )
                            .unwrap();
                        }
                        "lock" => {
                            assert!(fixture.import().unwrap_err().to_string().contains("active"));
                        }
                        _ => unreachable!(),
                    }
                    if scenario == "lock" {
                        Err(invalid("stop outer fixture before publication"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(result.is_err(), "{scenario}");
            if scenario == "collision" {
                assert_eq!(
                    fs::read(fixture.destination.join("sentinel")).unwrap(),
                    b"preserve a concurrent destination"
                );
                fs::remove_dir_all(&fixture.destination).unwrap();
            }
            fixture.no_partial_profile();
        }
    }

    #[test]
    fn existing_destinations_nested_sources_and_links_are_not_followed_or_overwritten() {
        let fixture = Fixture::new();
        fs::create_dir(&fixture.destination).unwrap();
        assert!(
            fixture.import().is_err(),
            "even an empty existing destination is rejected"
        );
        assert!(
            import_shipping_profile(
                &fixture.settings,
                &fixture.data,
                &fixture.data.join("nested"),
                &CancelToken::default()
            )
            .is_err()
        );
        #[cfg(unix)]
        {
            let nested = fixture.data.join("capture-history/.external-link");
            std::os::unix::fs::symlink(&fixture.settings, &nested).unwrap();
            fs::remove_dir(&fixture.destination).unwrap();
            assert!(fixture.import().unwrap_err().to_string().contains("link"));
            fixture.no_partial_profile();
            fs::remove_file(nested).unwrap();
            std::os::unix::fs::symlink(&fixture.data, &fixture.destination).unwrap();
            assert!(fixture.import().is_err());
            assert!(fixture.destination.is_symlink());
        }
    }

    #[test]
    fn pre_update_snapshot_preserves_working_bytes_without_migration_or_permission_reset() {
        let fixture = Fixture::new();
        fixture.import().unwrap();
        let settings = fixture.destination.join("settings.json");
        let mut value = super::json(&settings).unwrap();
        value["settings_schema_version"] = 0.into();
        value["onboarding_completed"] = true.into();
        value["launch_at_login"] = true.into();
        value["output_directory"] = fixture
            .root
            .path()
            .join("external exports")
            .to_str()
            .unwrap()
            .into();
        value["future_setting"] = json!({"keep": [17, 3, 91]});
        fs::write(&settings, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        fs::write(fixture.destination.join("startup.log"), b"old startup log").unwrap();
        let diagnostic = fixture.destination.join("history/.crash-diagnostics");
        fs::create_dir(&diagnostic).unwrap();
        fs::write(diagnostic.join("local.json"), b"retained local diagnostics").unwrap();
        let paths = [
            PathBuf::from("settings.json"),
            PathBuf::from("history")
                .join(&fixture.capture_id)
                .join("capture.png"),
            PathBuf::from("editor-drafts")
                .join(&fixture.capture_id)
                .join("manifest.json"),
            PathBuf::from("recording-recovery")
                .join(&fixture.recovery_id)
                .join("segment-003.mp4"),
            PathBuf::from("history/.crash-diagnostics/local.json"),
            PathBuf::from(DEVELOPMENT_MARKER),
        ];
        let before: Vec<_> = paths
            .iter()
            .map(|path| fs::read(fixture.destination.join(path)).unwrap())
            .collect();
        let prepared =
            prepare_development_profile(&fixture.destination, &CancelToken::default()).unwrap();
        prepared.verify(&CancelToken::default()).unwrap();
        let snapshot = prepared.snapshot().to_owned();
        for (path, expected) in paths.iter().zip(&before) {
            assert_eq!(fs::read(fixture.destination.join(path)).unwrap(), *expected);
            assert_eq!(fs::read(snapshot.join(path)).unwrap(), *expected);
        }
        assert!(snapshot.join("history/.retained/empty é").is_dir());
        for excluded in [
            "startup.log",
            "source-snapshot",
            "exports",
            "account-session.json",
        ] {
            assert!(!snapshot.join(excluded).exists(), "{excluded}");
        }
        let receipt = super::json(&snapshot.join("snapshot-receipt.json")).unwrap();
        assert_eq!(
            receipt["source_root"],
            fixture
                .destination
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
        );
        drop(prepared);
        assert!(
            snapshot.is_dir(),
            "dropping preparation must not discard rollback data"
        );
    }

    #[test]
    fn snapshot_reverification_rejects_changed_source_snapshot_and_empty_directory_layout() {
        for (snapshot, change) in [(false, "bytes"), (true, "bytes"), (false, "directory")] {
            let fixture = Fixture::new();
            fixture.import().unwrap();
            let prepared =
                prepare_development_profile(&fixture.destination, &CancelToken::default()).unwrap();
            let root = if snapshot {
                prepared.snapshot()
            } else {
                prepared.root()
            };
            if change == "directory" {
                fs::remove_dir(root.join("history/.retained/empty é")).unwrap();
            } else {
                fs::write(
                    root.join("history")
                        .join(&fixture.capture_id)
                        .join("capture.png"),
                    b"changed bytes",
                )
                .unwrap();
            }
            assert!(prepared.verify(&CancelToken::default()).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_root_alias_cannot_substitute_for_independent_rollback_data() {
        let fixture = Fixture::new();
        fixture.import().unwrap();
        let prepared =
            prepare_development_profile(&fixture.destination, &CancelToken::default()).unwrap();
        fs::remove_dir_all(prepared.snapshot()).unwrap();
        std::os::unix::fs::symlink(prepared.root(), prepared.snapshot()).unwrap();
        // Every content digest still matches, but these are now the SAME files.
        assert!(prepared.verify(&CancelToken::default()).is_err());
    }

    #[test]
    fn development_marker_rejects_unenrolled_copied_foreign_and_oversized_profiles() {
        let fixture = Fixture::new();
        fixture.import().unwrap();
        let marker = fixture.destination.join(DEVELOPMENT_MARKER);
        let original = fs::read(&marker).unwrap();
        assert_eq!(
            validate_development_profile(&fixture.destination).unwrap(),
            fixture.destination.canonicalize().unwrap()
        );
        let settings_path = fixture.destination.join("settings.json");
        let settings_bytes = fs::read(&settings_path).unwrap();
        let mut future_settings = super::json(&settings_path).unwrap();
        future_settings["settings_schema_version"] =
            json!(captures_settings::CURRENT_SETTINGS_SCHEMA_VERSION + 1);
        let future_bytes = serde_json::to_vec(&future_settings).unwrap();
        fs::write(&settings_path, &future_bytes).unwrap();
        assert!(
            prepare_development_profile(&fixture.destination, &CancelToken::default())
                .err()
                .unwrap()
                .to_string()
                .contains("newer unsupported schema")
        );
        assert_eq!(fs::read(&settings_path).unwrap(), future_bytes);
        fs::write(&settings_path, settings_bytes).unwrap();
        let other = fixture.root.path().join("copied profile");
        fs::create_dir(&other).unwrap();
        fs::copy(&marker, other.join(DEVELOPMENT_MARKER)).unwrap();
        fs::copy(
            fixture.destination.join("settings.json"),
            other.join("settings.json"),
        )
        .unwrap();
        assert!(validate_development_profile(&other).is_err());
        for (field, value) in [
            ("schema_version", json!(2)),
            ("identity", json!("shipping")),
            ("unknown", json!(true)),
        ] {
            let mut value_json: serde_json::Value = serde_json::from_slice(&original).unwrap();
            value_json[field] = value;
            fs::write(&marker, serde_json::to_vec(&value_json).unwrap()).unwrap();
            assert!(
                validate_development_profile(&fixture.destination).is_err(),
                "{field}"
            );
        }
        File::create(&marker)
            .unwrap()
            .set_len(MAX_JSON_BYTES + 1)
            .unwrap();
        assert!(validate_development_profile(&fixture.destination).is_err());
        fs::remove_file(marker).unwrap();
        assert!(validate_development_profile(&fixture.destination).is_err());
    }

    #[test]
    fn cancelled_or_unsafe_snapshot_preparation_leaves_no_partial_snapshot() {
        let fixture = Fixture::new();
        fixture.import().unwrap();
        let cancel = CancelToken::default();
        cancel.cancel();
        assert!(matches!(
            prepare_development_profile(&fixture.destination, &cancel),
            Err(Error::Cancelled)
        ));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                &fixture.settings,
                fixture.destination.join("history/foreign-link"),
            )
            .unwrap();
            assert!(
                prepare_development_profile(&fixture.destination, &CancelToken::default()).is_err()
            );
        }
        assert!(!fs::read_dir(fixture.root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".captures-native-pre-update-")
        }));
        assert!(fixture.destination.is_dir());
    }

    #[test]
    fn tree_digest_frames_paths_empty_directories_and_enforces_limits_before_reading() {
        let root = tempfile::tempdir().unwrap();
        let left = root.path().join("left");
        let right = root.path().join("right");
        fs::create_dir(&left).unwrap();
        fs::create_dir(&right).unwrap();
        fs::write(left.join("ab"), b"c").unwrap();
        fs::write(right.join("a"), b"bc").unwrap();
        let hash = |path: &Path| {
            tree(path, None, &CancelToken::default(), &mut Budget::default()).unwrap()
        };
        assert_ne!(
            hash(&left),
            hash(&right),
            "unframed path/content concatenation would collide"
        );
        fs::remove_file(right.join("a")).unwrap();
        fs::write(right.join("ab"), b"c").unwrap();
        assert_eq!(hash(&left), hash(&right));
        fs::create_dir(right.join("empty")).unwrap();
        assert_ne!(hash(&left), hash(&right));
        for (remaining, accepted) in [(1, true), (0, false)] {
            assert_eq!(
                tree(
                    &left,
                    None,
                    &CancelToken::default(),
                    &mut Budget {
                        entries: 0,
                        bytes: MAX_BYTES - remaining
                    }
                )
                .is_ok(),
                accepted,
                "byte limit is inclusive"
            );
        }
        // Windows set_len may allocate the entire file without an explicit
        // sparse flag. Keep its limit test above independent of disk capacity.
        #[cfg(unix)]
        {
            let sparse = root.path().join("oversized");
            File::create(&sparse)
                .unwrap()
                .set_len(MAX_BYTES + 1)
                .unwrap();
            assert!(
                tree(
                    &sparse,
                    None,
                    &CancelToken::default(),
                    &mut Budget::default()
                )
                .is_err()
            );
        }
        assert!(
            tree(
                &left,
                None,
                &CancelToken::default(),
                &mut Budget {
                    entries: MAX_ENTRIES,
                    bytes: 0
                }
            )
            .is_err()
        );
    }
}
