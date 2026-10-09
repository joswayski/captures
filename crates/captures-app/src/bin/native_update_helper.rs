//! Explicit stopped-development-package replacement, never a shipping updater.
use std::{fs, path::PathBuf, time::Duration};

use captures_app::{
    profile_import::{prepare_development_profile, validate_development_profile},
    updater::{Renderer, ShutdownIntent, Target, UpdateClient, recover_installation},
};
use captures_media::CancelToken;
use serde_json::json;

const USAGE: &str = "usage: native_update_helper --manifest-url URL --public-key-file PATH --current-version VERSION --renderer appkit|wgpu --stopped-development-package ABSOLUTE_PATH (--empty-test-profile ABSOLUTE_PATH | --new-development-profile ABSOLUTE_PATH --source-settings-file ABSOLUTE_PATH --source-data-directory ABSOLUTE_PATH --all-app-processes-stopped | --existing-development-profile ABSOLUTE_PATH --all-app-processes-stopped) [--health-timeout-seconds 1..120] [--restore-preferences true|false | --shutdown-intent-file ABSOLUTE_PATH (existing profile only)] [--base-archive ABSOLUTE_PATH]\nor: native_update_helper --recover-stopped-development-package ABSOLUTE_PATH --all-app-processes-stopped";

fn main() {
    if let Err(error) = run(std::env::args().skip(1)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run(arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut arguments = arguments.peekable();
    if arguments
        .peek()
        .is_some_and(|flag| flag == "--recover-stopped-development-package")
    {
        arguments.next();
        let destination = PathBuf::from(arguments.next().ok_or(USAGE)?);
        if !destination.is_absolute()
            || arguments.next().as_deref() != Some("--all-app-processes-stopped")
            || arguments.next().is_some()
        {
            return Err(USAGE.into());
        }
        // This is an operator assertion, not process-tree detection. Never
        // kill a process or infer quiescence from the original root's exit.
        // Do not canonicalize the final component: the core rejects links and
        // can restore a missing destination at an interrupted rename boundary.
        let changed = recover_installation(&destination).map_err(|error| error.to_string())?;
        println!(
            "{}",
            json!({"state":if changed { "recovery_complete" } else { "no_pending_replacement" }, "changed":changed})
        );
        return Ok(());
    }
    let mut endpoint = None;
    let mut key = None;
    let mut version = None;
    let mut renderer = None;
    let mut destination = None;
    let mut profile = None;
    let mut new_profile = None;
    let mut existing_profile = None;
    let mut source_settings = None;
    let mut source_data = None;
    let mut stopped = false;
    let mut restore_preferences = None;
    let mut shutdown_intent = None;
    let mut base_archive = None;
    let mut timeout = Duration::from_secs(60);
    while let Some(argument) = arguments.next() {
        if argument == "--all-app-processes-stopped" {
            if stopped {
                return Err(USAGE.into());
            }
            stopped = true;
            continue;
        }
        let value = arguments.next().ok_or(USAGE)?;
        match argument.as_str() {
            "--manifest-url" => endpoint = Some(value),
            "--public-key-file" => key = Some(PathBuf::from(value)),
            "--current-version" => version = Some(value),
            "--renderer" => {
                renderer = Some(match value.as_str() {
                    "appkit" => Renderer::Appkit,
                    "wgpu" => Renderer::Wgpu,
                    _ => return Err("Renderer must be appkit or wgpu.".into()),
                })
            }
            "--stopped-development-package" => destination = Some(PathBuf::from(value)),
            "--empty-test-profile" => profile = Some(PathBuf::from(value)),
            "--new-development-profile" if new_profile.is_none() => {
                new_profile = Some(PathBuf::from(value))
            }
            "--existing-development-profile" if existing_profile.is_none() => {
                existing_profile = Some(PathBuf::from(value))
            }
            "--source-settings-file" if source_settings.is_none() => {
                source_settings = Some(PathBuf::from(value))
            }
            "--source-data-directory" if source_data.is_none() => {
                source_data = Some(PathBuf::from(value))
            }
            "--restore-preferences" if restore_preferences.is_none() => {
                restore_preferences = Some(value.parse::<bool>().map_err(|_| USAGE)?);
            }
            "--shutdown-intent-file" if shutdown_intent.is_none() => {
                shutdown_intent = Some(PathBuf::from(value));
            }
            "--base-archive" if base_archive.is_none() => base_archive = Some(PathBuf::from(value)),
            "--health-timeout-seconds" => {
                let seconds: u64 = value.parse().map_err(|_| USAGE)?;
                if !(1..=120).contains(&seconds) {
                    return Err(USAGE.into());
                }
                timeout = Duration::from_secs(seconds);
            }
            _ => return Err(format!("Unknown argument {argument}. {USAGE}")),
        }
    }
    let endpoint = endpoint.ok_or(USAGE)?;
    let key = key.ok_or(USAGE)?;
    let version = version.ok_or(USAGE)?;
    let renderer = renderer.ok_or(USAGE)?;
    let destination = destination.ok_or(USAGE)?;
    let (profile, sources, reuse) = match (
        profile,
        new_profile,
        existing_profile,
        source_settings,
        source_data,
        stopped,
    ) {
        (Some(profile), None, None, None, None, _) => (profile, None, false),
        (None, Some(profile), None, Some(settings), Some(data), true) => {
            (profile, Some((settings, data)), false)
        }
        (None, None, Some(profile), None, None, true) => (profile, None, true),
        _ => return Err(USAGE.into()),
    };
    if shutdown_intent.is_some() && (!reuse || restore_preferences.is_some()) {
        return Err("Shutdown intent requires an existing enrolled development profile and cannot override --restore-preferences.".into());
    }
    if !destination.is_absolute() || !profile.is_absolute() {
        return Err("Package and development profile must be explicit absolute paths.".into());
    }
    if !fs::symlink_metadata(&destination)
        .map_err(|error| error.to_string())?
        .is_dir()
    {
        return Err("The development package must be an existing directory, not a link.".into());
    }
    let destination = fs::canonicalize(destination).map_err(|error| error.to_string())?;
    let profile = if reuse {
        validate_development_profile(&profile).map_err(|error| error.to_string())?
    } else if let Some((settings, data)) = &sources {
        let name = profile.file_name().ok_or(USAGE)?;
        let profile = profile
            .parent()
            .ok_or(USAGE)?
            .canonicalize()
            .map_err(|error| error.to_string())?
            .join(name);
        match fs::symlink_metadata(&profile) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
            Ok(_) => return Err("The imported development profile must not exist.".into()),
        }
        for (source, is_directory) in [(settings, false), (data, true)] {
            if !source.is_absolute() {
                return Err("Import sources must be explicit absolute paths.".into());
            }
            let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
            if metadata.file_type().is_symlink()
                || if is_directory {
                    !metadata.is_dir()
                } else {
                    !metadata.is_file()
                }
            {
                return Err("Import sources must be real settings/data, not links.".into());
            }
            let source = source.canonicalize().map_err(|error| error.to_string())?;
            if source.starts_with(&destination) || destination.starts_with(&source) {
                return Err("Import sources must stay outside the development package.".into());
            }
            if is_directory && profile.starts_with(&source) {
                return Err("The imported profile must stay outside source data.".into());
            }
        }
        profile
    } else {
        if !fs::symlink_metadata(&profile)
            .map_err(|error| error.to_string())?
            .is_dir()
            || fs::read_dir(&profile)
                .map_err(|error| error.to_string())?
                .next()
                .is_some()
        {
            return Err("The test profile must be an existing empty directory, not a link.".into());
        }
        fs::canonicalize(profile).map_err(|error| error.to_string())?
    };
    if profile.starts_with(&destination) || (reuse && destination.starts_with(&profile)) {
        return Err("The test profile must stay outside the development package.".into());
    }
    let target = Target::current_host().ok_or("This native update target is not supported.")?;
    let key = fs::read_to_string(key).map_err(|error| error.to_string())?;
    let client = UpdateClient::new(&endpoint, &key, renderer, target, &version)
        .map_err(|error| error.to_string())?;
    let client = match base_archive {
        Some(path) => client
            .with_base_archive(path)
            .map_err(|error| error.to_string())?,
        None => client,
    };
    let cancel = CancelToken::default();
    let Some(update) = client.check(&cancel).map_err(|error| error.to_string())? else {
        println!("{}", json!({"state":"up_to_date","replaced":false}));
        return Ok(());
    };
    if let Some(path) = &shutdown_intent {
        // A GUI record is only visibility intent, never an authenticated update
        // or proof of stopped writers. Reauthenticate and bind the exact target
        // before any package download, profile snapshot, replacement or launch.
        restore_preferences = Some(
            ShutdownIntent::take_matching(path, &profile, &update)
                .map_err(|error| error.to_string())?,
        );
    }
    let scratch = tempfile::tempdir_in(destination.parent().ok_or(USAGE)?)
        .map_err(|error| error.to_string())?;
    let staged = update
        .download(scratch.path(), &cancel, |_, _| {})
        .and_then(|verified| verified.stage(scratch.path(), &cancel))
        .map_err(|error| error.to_string())?;
    let release = staged.info().clone();
    // Snapshot before activation: a launched newer host can migrate working data.
    // Package rollback alone cannot undo those writes.
    let prepared = if reuse {
        Some(prepare_development_profile(&profile, &cancel).map_err(|error| error.to_string())?)
    } else {
        None
    };
    let snapshot = prepared.as_ref().map(|profile| profile.snapshot());
    let retained_error = |error: String| match snapshot {
        Some(snapshot) => format!(
            "{error} Pre-update profile snapshot retained at {}.",
            snapshot.display()
        ),
        None => error,
    };
    if let Some(prepared) = &prepared {
        prepared
            .verify(&cancel)
            .map_err(|error| retained_error(error.to_string()))?;
    }
    let pending = staged
        .replace(&destination, &cancel)
        .map_err(|error| retained_error(error.to_string()))?;
    let pending = match restore_preferences {
        Some(visible) => pending.with_restart_preferences(visible),
        None => pending,
    };
    let launched = if let Some(prepared) = &prepared {
        pending.launch_existing(prepared, timeout, &cancel)
    } else if let Some((settings, data)) = &sources {
        pending.launch_importing(settings, data, &profile, timeout, &cancel)
    } else {
        pending.launch(&profile, timeout, &cancel)
    };
    match launched {
        Ok(child) => {
            println!(
                "{}",
                json!({"state":"confirmed","release":release,"process_id":child.id(),
                "profile_snapshot":snapshot,
                "profile":if reuse { "existing enrolled development profile; pre-update data snapshot retained" }
                    else if sources.is_some() { "new isolated imported development profile; shipping sources unchanged" }
                    else { "new disposable test profile; no installed data imported" }})
            );
            // Dropping Child leaves the acknowledged GUI running; there is no pipe
            // whose closure could break its subsequent diagnostics/output.
            Ok(())
        }
        Err(failure) => {
            println!(
                "{}",
                json!({"state":"handoff_failed","process_id":failure.process.as_ref().map(|child| child.id()),
                "profile_snapshot":snapshot,
                "manual_recovery":true,"recovery_requires_all_app_processes_stopped":true})
            );
            Err(format!(
                "{} No automatic rollback was attempted. Stop every app process before explicit recovery; inspect the retained transaction and profile startup*.log if created. Published profile copies/snapshots are retained; package recovery does not restore profile data.",
                failure.error
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importing_requires_complete_exclusive_paths_and_stopped_writer_assertion() {
        let base = [
            "--manifest-url",
            "https://example.invalid/manifest",
            "--public-key-file",
            "/key",
            "--current-version",
            "1.0.0",
            "--renderer",
            "wgpu",
            "--stopped-development-package",
            "/package",
        ];
        for flags in [
            vec![
                "--new-development-profile",
                "/new",
                "--source-data-directory",
                "/data",
                "--all-app-processes-stopped",
            ],
            vec![
                "--new-development-profile",
                "/new",
                "--source-settings-file",
                "/settings",
                "--all-app-processes-stopped",
            ],
            vec![
                "--new-development-profile",
                "/new",
                "--source-settings-file",
                "/settings",
                "--source-data-directory",
                "/data",
            ],
            vec![
                "--empty-test-profile",
                "/empty",
                "--new-development-profile",
                "/new",
                "--source-settings-file",
                "/settings",
                "--source-data-directory",
                "/data",
                "--all-app-processes-stopped",
            ],
            vec![
                "--empty-test-profile",
                "/empty",
                "--source-settings-file",
                "/settings",
            ],
            vec![
                "--new-development-profile",
                "/new",
                "--new-development-profile",
                "/other",
            ],
            vec![
                "--empty-test-profile",
                "/empty",
                "--restore-preferences",
                "1",
            ],
            vec![
                "--empty-test-profile",
                "/empty",
                "--restore-preferences",
                "true",
                "--restore-preferences",
                "false",
            ],
            vec!["--existing-development-profile", "/existing"],
            vec![
                "--existing-development-profile",
                "/existing",
                "--empty-test-profile",
                "/empty",
                "--all-app-processes-stopped",
            ],
            vec![
                "--existing-development-profile",
                "/existing",
                "--source-settings-file",
                "/settings",
                "--all-app-processes-stopped",
            ],
            vec![
                "--existing-development-profile",
                "/existing",
                "--existing-development-profile",
                "/other",
                "--all-app-processes-stopped",
            ],
            vec!["--all-app-processes-stopped", "--all-app-processes-stopped"],
        ] {
            assert!(
                run(base.into_iter().chain(flags).map(String::from))
                    .unwrap_err()
                    .contains(USAGE)
            );
        }
    }

    #[test]
    fn existing_mode_rejects_unenrolled_profiles_before_network_or_replacement() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("package");
        let profile = root.path().join("arbitrary profile");
        fs::create_dir(&package).unwrap();
        fs::create_dir(&profile).unwrap();
        fs::write(package.join("sentinel"), b"keep original package").unwrap();
        let settings = serde_json::to_vec(&captures_settings::AppSettings::default()).unwrap();
        fs::write(profile.join("settings.json"), &settings).unwrap();
        let error = run([
            "--manifest-url".into(),
            "https://example.invalid/manifest".into(),
            "--public-key-file".into(),
            root.path()
                .join("nonexistent-key")
                .to_string_lossy()
                .into_owned(),
            "--current-version".into(),
            "1.0.0".into(),
            "--renderer".into(),
            "wgpu".into(),
            "--stopped-development-package".into(),
            package.to_string_lossy().into_owned(),
            "--existing-development-profile".into(),
            profile.to_string_lossy().into_owned(),
            "--all-app-processes-stopped".into(),
        ]
        .into_iter())
        .unwrap_err();
        assert!(error.contains("not enrolled"), "{error}");
        assert_eq!(
            fs::read(package.join("sentinel")).unwrap(),
            b"keep original package"
        );
        assert_eq!(fs::read(profile.join("settings.json")).unwrap(), settings);
        assert_eq!(
            fs::read_dir(&profile).unwrap().count(),
            1,
            "must not enroll arbitrary data"
        );
    }

    #[test]
    fn unsafe_import_paths_fail_before_acquisition_or_package_replacement() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("package");
        let data = root.path().join("shipping data");
        let settings = root.path().join("shipping settings.json");
        fs::create_dir(&package).unwrap();
        fs::create_dir(&data).unwrap();
        fs::write(package.join("sentinel"), b"package must not move").unwrap();
        fs::write(&settings, b"source must not change").unwrap();
        for scenario in [
            "existing",
            "data-in-package",
            "profile-in-source",
            "relative-source",
        ] {
            let mut profile = root.path().join("new profile");
            let mut source = data.clone();
            match scenario {
                "existing" => fs::create_dir(&profile).unwrap(),
                "data-in-package" => source = package.clone(),
                "profile-in-source" => profile = data.join("new profile"),
                "relative-source" => source = PathBuf::from("relative-data"),
                _ => unreachable!(),
            }
            let arguments = [
                "--manifest-url".into(),
                "https://example.invalid/manifest".into(),
                "--public-key-file".into(),
                root.path()
                    .join("missing-key")
                    .to_string_lossy()
                    .into_owned(),
                "--current-version".into(),
                "1.0.0".into(),
                "--renderer".into(),
                "wgpu".into(),
                "--stopped-development-package".into(),
                package.to_string_lossy().into_owned(),
                "--new-development-profile".into(),
                profile.to_string_lossy().into_owned(),
                "--source-settings-file".into(),
                settings.to_string_lossy().into_owned(),
                "--source-data-directory".into(),
                source.to_string_lossy().into_owned(),
                "--all-app-processes-stopped".into(),
            ];
            let error = run(arguments.into_iter()).unwrap_err();
            assert!(
                !error.contains("No such file") && !error.contains("cannot find"),
                "{scenario}: {error}"
            );
            assert!(
                error.contains("must not exist")
                    || error.contains("outside")
                    || error.contains("absolute"),
                "{scenario}: {error}"
            );
            assert_eq!(
                fs::read(package.join("sentinel")).unwrap(),
                b"package must not move"
            );
            assert_eq!(fs::read(&settings).unwrap(), b"source must not change");
            if scenario == "existing" {
                fs::remove_dir(profile).unwrap();
            } else {
                assert!(!profile.exists(), "{scenario}");
            }
        }
    }

    #[test]
    fn helper_never_defaults_to_an_installed_package_or_profile() {
        assert!(
            run([].into_iter())
                .unwrap_err()
                .contains("--stopped-development-package")
        );
        for seconds in ["0", "121", "bad"] {
            assert!(
                run(["--health-timeout-seconds", seconds]
                    .map(String::from)
                    .into_iter())
                .is_err()
            );
        }
        assert!(
            run(["--installed-profile", "automatic"]
                .map(String::from)
                .into_iter())
            .is_err()
        );
    }

    #[test]
    fn recovery_requires_exact_mode_absolute_destination_and_stopped_process_assertion() {
        for args in [
            vec!["--recover-stopped-development-package"],
            vec!["--recover-stopped-development-package", "/package"],
            vec![
                "--recover-stopped-development-package",
                "relative",
                "--all-app-processes-stopped",
            ],
            vec![
                "--recover-stopped-development-package",
                "/package",
                "--root-process-exited",
            ],
            vec![
                "--recover-stopped-development-package",
                "/package",
                "--all-app-processes-stopped",
                "--manifest-url",
                "https://example.invalid",
            ],
        ] {
            assert!(run(args.into_iter().map(String::from)).is_err());
        }
    }

    #[test]
    fn recovery_without_pending_work_preserves_files_and_never_needs_a_release_endpoint() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("development package é");
        fs::create_dir(&package).unwrap();
        fs::write(
            package.join("sentinel"),
            b"preserve this asymmetric content",
        )
        .unwrap();
        let args = |destination: &std::path::Path| {
            [
                "--recover-stopped-development-package".into(),
                destination.to_string_lossy().into_owned(),
                "--all-app-processes-stopped".into(),
            ]
            .into_iter()
        };
        run(args(&package)).unwrap();
        assert_eq!(
            fs::read(package.join("sentinel")).unwrap(),
            b"preserve this asymmetric content"
        );
        // Missing package is legitimate after the first activation rename.
        run(args(&root.path().join("missing-development-package"))).unwrap();
        #[cfg(unix)]
        {
            let link = root.path().join("package-link");
            std::os::unix::fs::symlink(&package, &link).unwrap();
            assert!(run(args(&link)).is_err());
            assert_eq!(
                fs::read(package.join("sentinel")).unwrap(),
                b"preserve this asymmetric content"
            );
        }
    }
}
