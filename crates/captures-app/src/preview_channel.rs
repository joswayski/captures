//! Installed native Preview identity and first-run data handoff. Development
//! workbenches never opt into this channel simply by being built in release mode.
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

use captures_media::CancelToken;
use serde::{Deserialize, Serialize};

pub const IDENTITY: &str = "io.github.joswayski.captures";
pub const PROFILE_MARKER: &str = ".captures-native-preview-profile.json";

pub const fn version() -> Option<&'static str> {
    option_env!("CAPTURES_NATIVE_PREVIEW_VERSION")
}

pub fn public_key() -> Option<&'static str> {
    option_env!("CAPTURES_NATIVE_PREVIEW_PUBLIC_KEY")
}

/// Desktop launchers using %U pass local file URIs, not filesystem paths.
pub fn local_media_path(argument: PathBuf) -> Result<PathBuf, String> {
    if let Some(value) = argument.to_str().filter(|value| value.starts_with("file:")) {
        return reqwest::Url::parse(value)
            .map_err(|_| "Invalid local file URL".to_owned())?
            .to_file_path()
            .map_err(|_| "Open With accepts only local file URLs".to_owned());
    }
    Ok(argument)
}

#[derive(Debug, Serialize)]
pub struct Startup {
    pub version: String,
    pub settings_file: PathBuf,
    pub history_root: PathBuf,
    pub imported: bool,
}

/// Kept for the entire host lifetime, including all worker shutdown. An update
/// helper takes the exclusive counterpart before touching installed binaries.
pub fn package_use() -> Result<Option<File>, String> {
    if version().is_none() {
        return Ok(None);
    }
    let file = package_lock()?;
    file.try_lock_shared()
        .map_err(|_| "A Captures update is being installed. Try again shortly.")?;
    Ok(Some(file))
}

pub fn package_lock() -> Result<File, String> {
    let profile = profile_path()?;
    let parent = profile
        .parent()
        .ok_or("Native Preview profile has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let path = parent.join("package-use.lock");
    regular_or_absent(&path)?;
    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| e.to_string())
}

pub fn profile_path() -> Result<PathBuf, String> {
    std::path::absolute(captures_settings::native_preview_profile_path()).map_err(|e| e.to_string())
}

/// Called before profile election. The import lock serializes concurrent cold
/// launches; a published profile is validated, never overwritten or reimported.
pub fn startup() -> Result<Option<Startup>, String> {
    let Some(version) = version() else {
        return Ok(None);
    };
    semver::Version::parse(version).map_err(|_| "Invalid installed Preview version")?;
    let destination = profile_path()?;
    let settings = std::path::absolute(captures_settings::shipping_settings_path())
        .map_err(|e| e.to_string())?;
    let data = captures_settings::shipping_data_directory()
        .ok_or("Could not locate the previous Captures data directory")?;
    let imported = prepare_profile(&settings, &data, &destination, ensure_previous_app_stopped)?;
    if let Ok(entries) = fs::read_dir(destination.join("updates")) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("captures-preview-")
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry.path().join("completed").is_file()
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(Some(Startup {
        version: version.into(),
        settings_file: destination.join("settings.json"),
        history_root: destination.join("history"),
        imported,
    }))
}

fn regular_or_absent(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
        Ok(_) => Err("Native Preview metadata must be a regular file, not a link".into()),
    }
}

fn prepare_profile(
    settings: &Path,
    data: &Path,
    destination: &Path,
    require_stopped: impl Fn() -> Result<(), String>,
) -> Result<bool, String> {
    let parent = destination
        .parent()
        .ok_or("Preview profile has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let lock_path = parent.join("first-launch.lock");
    regular_or_absent(&lock_path)?;
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            validate_profile(destination)?;
            return Ok(false);
        }
        Ok(_) => return Err("Native Preview profile is not a real directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    // Missing settings and data are normal on a new install. Supply private
    // defaults without creating or changing anything in the previous profile.
    let defaults = tempfile::tempdir().map_err(|e| e.to_string())?;
    let settings_source = match fs::symlink_metadata(settings) {
        Ok(_) => {
            require_stopped()?;
            settings.to_owned()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let path = defaults.path().join("settings.json");
            captures_settings::write_atomic(&path, &captures_settings::AppSettings::default())
                .map_err(|e| e.to_string())?;
            path
        }
        Err(error) => return Err(error.to_string()),
    };
    let data_source = match fs::symlink_metadata(data) {
        Ok(_) => {
            require_stopped()?;
            data.to_owned()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let path = defaults.path().join("data");
            fs::create_dir(&path).map_err(|e| e.to_string())?;
            path
        }
        Err(error) => return Err(error.to_string()),
    };
    crate::profile_import::import_preview_profile(
        &settings_source,
        &data_source,
        destination,
        &CancelToken::default(),
    )
    .map_err(|e| e.to_string())?;
    validate_profile(destination)?;
    Ok(settings_source == settings || data_source == data)
}

fn validate_profile(root: &Path) -> Result<(), String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Marker {
        schema_version: u8,
        identity: String,
        root: PathBuf,
    }
    let marker_path = root.join(PROFILE_MARKER);
    regular_or_absent(&marker_path)?;
    let bytes = fs::read(&marker_path).map_err(|e| e.to_string())?;
    if bytes.len() > 16 * 1024 {
        return Err("Preview profile marker is too large".into());
    }
    let marker: Marker = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if marker.schema_version != 1
        || marker.identity != IDENTITY
        || marker.root != root.canonicalize().map_err(|e| e.to_string())?
    {
        return Err("Preview profile belongs to a different location or application".into());
    }
    regular_or_absent(&root.join("settings.json"))?;
    if !root.join("settings.json").is_file() {
        return Err("Preview settings are missing".into());
    }
    Ok(())
}

fn ensure_previous_app_stopped() -> Result<(), String> {
    // The installed binary name is deliberately preserved across the cutover.
    // Only the first import needs this additional check: later launches use an
    // isolated profile and the native primary-instance protocol.
    #[cfg(windows)]
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-Process -Name captures -ErrorAction SilentlyContinue | ForEach-Object { $_.Id }",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    let output = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,comm="])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Could not confirm that the previous Captures app is stopped".into());
    }
    let text = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        #[cfg(not(windows))]
        if !fields
            .next()
            .and_then(|value| Path::new(value).file_name())
            .is_some_and(|name| name == "captures" || name == "Captures")
        {
            continue;
        }
        if pid != std::process::id() {
            return Err("Quit every other Captures app, then open Captures again to import your captures safely.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_handoff_preserves_preferences_and_never_reimports_existing_data() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("old-settings.json");
        let data = temp.path().join("old-data");
        let profile = temp.path().join("preview");
        fs::create_dir(&data).unwrap();
        let original = captures_settings::AppSettings {
            output_directory: temp
                .path()
                .join("my-exports")
                .to_string_lossy()
                .into_owned(),
            launch_at_login: true,
            onboarding_completed: true,
            ..Default::default()
        };
        captures_settings::write_atomic(&settings, &original).unwrap();
        let before = fs::read(&settings).unwrap();
        assert!(prepare_profile(&settings, &data, &profile, || Ok(())).unwrap());
        let mut migrated = captures_settings::load(&profile.join("settings.json")).unwrap();
        assert_eq!(migrated.output_directory, original.output_directory);
        assert!(migrated.launch_at_login && migrated.onboarding_completed);
        assert_eq!(fs::read(&settings).unwrap(), before);
        assert_eq!(
            fs::read(profile.join("source-snapshot/settings.json")).unwrap(),
            before
        );
        assert!(
            !profile
                .join(".captures-native-development-profile.json")
                .exists()
        );
        migrated.launch_at_login = false;
        captures_settings::write_atomic(&profile.join("settings.json"), &migrated).unwrap();
        assert!(!prepare_profile(&settings, &data, &profile, || panic!("not importing")).unwrap());
        assert!(
            !captures_settings::load(&profile.join("settings.json"))
                .unwrap()
                .launch_at_login
        );
    }

    #[test]
    fn fresh_install_never_creates_a_shipping_profile_and_rejects_unenrolled_destination() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("missing/settings.json");
        let data = temp.path().join("missing-data");
        let profile = temp.path().join("preview");
        assert!(!prepare_profile(&settings, &data, &profile, || panic!("no source")).unwrap());
        assert!(!settings.exists() && !data.exists());
        assert!(profile.join("settings.json").is_file());
        let invalid = temp.path().join("unrelated");
        fs::create_dir(&invalid).unwrap();
        fs::write(invalid.join("keep.txt"), b"not ours").unwrap();
        assert!(prepare_profile(&settings, &data, &invalid, || Ok(())).is_err());
        assert_eq!(fs::read(invalid.join("keep.txt")).unwrap(), b"not ours");
    }
}
