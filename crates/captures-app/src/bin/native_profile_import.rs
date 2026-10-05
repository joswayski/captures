//! Offline, explicit development-profile import; never an installed cutover.
use std::path::PathBuf;

const USAGE: &str = "usage: native_profile_import --source-settings-file ABSOLUTE_PATH --source-data-directory ABSOLUTE_PATH --new-development-profile ABSOLUTE_PATH --all-app-processes-stopped";

fn main() {
    if let Err(error) = run(std::env::args().skip(1)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut paths: [Option<PathBuf>; 3] = [None, None, None];
    let mut stopped = false;
    while let Some(argument) = arguments.next() {
        let index = match argument.as_str() {
            "--source-settings-file" => 0,
            "--source-data-directory" => 1,
            "--new-development-profile" => 2,
            "--all-app-processes-stopped" if !stopped => {
                stopped = true;
                continue;
            }
            _ => return Err(USAGE.into()),
        };
        if paths[index].is_some() {
            return Err(USAGE.into());
        }
        paths[index] = Some(arguments.next().ok_or(USAGE)?.into());
    }
    let [Some(settings), Some(data), Some(destination)] = paths else {
        return Err(USAGE.into());
    };
    if !stopped
        || [&settings, &data, &destination]
            .iter()
            .any(|path| !path.is_absolute())
    {
        return Err(USAGE.into());
    }
    let report = captures_app::profile_import::import_shipping_profile(
        &settings,
        &data,
        &destination,
        &captures_media::CancelToken::default(),
    )
    .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::json!({
            "state": "imported_development_profile", "source_unchanged": true,
            "copied_files": report.copied_files, "copied_bytes": report.copied_bytes,
            "history_root": destination.join("history"), "settings_file": destination.join("settings.json"),
            "snapshot": destination.join("source-snapshot")
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_cli_import_preserves_original_and_rejects_repeat_destination() {
        let root = tempfile::tempdir().unwrap();
        let settings = root.path().join("shipping settings é.json");
        let data = root.path().join("shipping data");
        // Exercise an aliased parent even on Linux, where /tmp usually is canonical.
        #[cfg(unix)]
        let destination = {
            let alias = root.path().join("parent alias");
            std::os::unix::fs::symlink(root.path(), &alias).unwrap();
            alias.join("native development é")
        };
        #[cfg(not(unix))]
        let destination = root.path().join("native development é");
        std::fs::create_dir(&data).unwrap();
        let original = serde_json::to_vec(&captures_settings::AppSettings {
            appearance: captures_settings::Appearance::Dark,
            onboarding_completed: true,
            launch_at_login: true,
            ..Default::default()
        })
        .unwrap();
        std::fs::write(&settings, &original).unwrap();
        let arguments = || {
            [
                "--source-settings-file".to_owned(),
                settings.to_str().unwrap().to_owned(),
                "--source-data-directory".to_owned(),
                data.to_str().unwrap().to_owned(),
                "--new-development-profile".to_owned(),
                destination.to_str().unwrap().to_owned(),
                "--all-app-processes-stopped".to_owned(),
            ]
            .into_iter()
        };
        run(arguments()).unwrap();
        assert_eq!(std::fs::read(&settings).unwrap(), original);
        assert_eq!(
            std::fs::read(destination.join("source-snapshot/settings.json")).unwrap(),
            original
        );
        let imported = captures_settings::load(&destination.join("settings.json")).unwrap();
        assert_eq!(imported.appearance, captures_settings::Appearance::Dark);
        assert!(!imported.onboarding_completed && !imported.launch_at_login);
        assert_eq!(
            PathBuf::from(imported.output_directory),
            destination.canonicalize().unwrap().join("exports")
        );
        assert!(run(arguments()).unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read(&settings).unwrap(), original);
    }

    #[test]
    fn import_has_no_installed_defaults_or_implicit_stopped_process_detection() {
        for arguments in [
            vec![],
            vec!["--all-app-processes-stopped"],
            vec![
                "--source-settings-file",
                "relative",
                "--source-data-directory",
                "/data",
                "--new-development-profile",
                "/profile",
                "--all-app-processes-stopped",
            ],
            vec![
                "--source-settings-file",
                "/settings",
                "--source-data-directory",
                "/data",
                "--new-development-profile",
                "/profile",
            ],
            vec![
                "--source-settings-file",
                "/settings",
                "--source-settings-file",
                "/other",
            ],
            vec!["--all-app-processes-stopped", "--all-app-processes-stopped"],
            vec!["--installed-profile", "automatic"],
            vec!["--root-process-exited"],
        ] {
            assert!(
                run(arguments.into_iter().map(String::from))
                    .unwrap_err()
                    .contains(USAGE)
            );
        }
    }
}
