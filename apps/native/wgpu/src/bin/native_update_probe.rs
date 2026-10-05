//! Explicit no-window update diagnostic. Never installs or relaunches anything.
use std::{fs, path::PathBuf};

use captures_app::updater::{Renderer, Target, UpdateClient};
use captures_media::CancelToken;
use serde_json::json;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut endpoint = None;
    let mut public_key = None;
    let mut current_version = None;
    let mut renderer = None;
    let mut download_directory = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| format!("Missing value for {argument}"))?;
        match argument.as_str() {
            "--manifest-url" => endpoint = Some(value),
            "--public-key-file" => public_key = Some(PathBuf::from(value)),
            "--current-version" => current_version = Some(value),
            "--renderer" => {
                renderer = Some(match value.as_str() {
                    "appkit" => Renderer::Appkit,
                    "wgpu" => Renderer::Wgpu,
                    _ => return Err("Renderer must be appkit or wgpu.".into()),
                })
            }
            "--download-directory" => download_directory = Some(PathBuf::from(value)),
            _ => return Err(format!("Unknown argument {argument}")),
        }
    }
    let usage = "usage: native_update_probe --manifest-url URL --public-key-file PATH --current-version VERSION --renderer appkit|wgpu [--download-directory EXISTING_DIRECTORY]";
    let endpoint = endpoint.ok_or(usage)?;
    let public_key =
        fs::read_to_string(public_key.ok_or(usage)?).map_err(|error| error.to_string())?;
    let current_version = current_version.ok_or(usage)?;
    let renderer = renderer.ok_or(usage)?;
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Target::MacArm64,
        ("macos", "x86_64") => Target::MacX64,
        ("windows", "x86_64") => Target::WindowsX64,
        ("linux", "x86_64") => Target::LinuxX64,
        _ => return Err("This native update target is not supported.".into()),
    };
    let client = UpdateClient::new(&endpoint, &public_key, renderer, target, &current_version)
        .map_err(|error| error.to_string())?;
    let cancel = CancelToken::default();
    match client.check(&cancel).map_err(|error| error.to_string())? {
        None => println!("{}", json!({"state": "up_to_date", "installed": false})),
        Some(update) => {
            if let Some(directory) = download_directory {
                let verified = update
                    .download(&directory, &cancel, |_, _| {})
                    .map_err(|error| error.to_string())?;
                println!(
                    "{}",
                    json!({"state": "verified", "release": verified.info(), "installed": false})
                );
                // Verified bytes remain temporary and are removed on normal exit.
            } else {
                println!(
                    "{}",
                    json!({"state": "available", "release": update.info(), "installed": false})
                );
            }
        }
    }
    Ok(())
}
