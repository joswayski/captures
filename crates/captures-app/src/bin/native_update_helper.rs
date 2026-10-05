//! Explicit stopped-development-package replacement, never a shipping updater.
use std::{fs, path::PathBuf, time::Duration};

use captures_app::updater::{Renderer, Target, UpdateClient};
use captures_media::CancelToken;
use serde_json::json;

const USAGE: &str = "usage: native_update_helper --manifest-url URL --public-key-file PATH --current-version VERSION --renderer appkit|wgpu --stopped-development-package ABSOLUTE_PATH --empty-test-profile ABSOLUTE_PATH [--health-timeout-seconds 1..120]";

fn main() {
    if let Err(error) = run(std::env::args().skip(1)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut endpoint = None;
    let mut key = None;
    let mut version = None;
    let mut renderer = None;
    let mut destination = None;
    let mut profile = None;
    let mut timeout = Duration::from_secs(60);
    while let Some(argument) = arguments.next() {
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
    let profile = profile.ok_or(USAGE)?;
    if !destination.is_absolute() || !profile.is_absolute() {
        return Err("Package and empty test profile must be explicit absolute paths.".into());
    }
    if !fs::symlink_metadata(&destination)
        .map_err(|error| error.to_string())?
        .is_dir()
    {
        return Err("The development package must be an existing directory, not a link.".into());
    }
    let destination = fs::canonicalize(destination).map_err(|error| error.to_string())?;
    if !fs::symlink_metadata(&profile)
        .map_err(|error| error.to_string())?
        .is_dir()
    {
        return Err("The test profile must be an existing empty directory, not a link.".into());
    }
    let profile = fs::canonicalize(profile).map_err(|error| error.to_string())?;
    if profile.starts_with(&destination)
        || fs::read_dir(&profile)
            .map_err(|error| error.to_string())?
            .next()
            .is_some()
    {
        return Err("The test profile must be empty and outside the development package.".into());
    }
    let target = Target::current_host().ok_or("This native update target is not supported.")?;
    let key = fs::read_to_string(key).map_err(|error| error.to_string())?;
    let client = UpdateClient::new(&endpoint, &key, renderer, target, &version)
        .map_err(|error| error.to_string())?;
    let cancel = CancelToken::default();
    let Some(update) = client.check(&cancel).map_err(|error| error.to_string())? else {
        println!("{}", json!({"state":"up_to_date","replaced":false}));
        return Ok(());
    };
    let scratch = tempfile::tempdir_in(destination.parent().ok_or(USAGE)?)
        .map_err(|error| error.to_string())?;
    let staged = update
        .download(scratch.path(), &cancel, |_, _| {})
        .and_then(|verified| verified.stage(scratch.path(), &cancel))
        .map_err(|error| error.to_string())?;
    let release = staged.info().clone();
    let pending = staged
        .replace(&destination, &cancel)
        .map_err(|error| error.to_string())?;
    match pending.launch(&profile, timeout, &cancel) {
        Ok(child) => {
            println!(
                "{}",
                json!({"state":"confirmed","release":release,"process_id":child.id(),
                "profile":"new disposable test profile; no installed data imported"})
            );
            // Dropping Child leaves the acknowledged GUI running; there is no pipe
            // whose closure could break its subsequent diagnostics/output.
            Ok(())
        }
        Err(failure) => {
            println!(
                "{}",
                json!({"state":"handoff_failed","process_id":failure.process.as_ref().map(|child| child.id()),
                "manual_recovery":true,"recovery_requires_all_app_processes_stopped":true})
            );
            Err(format!(
                "{} No automatic rollback was attempted. Stop every app process before explicit recovery; inspect the retained transaction and test profile startup.log.",
                failure.error
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
