//! Installed Preview helper, copied outside the package before the GUI exits.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use captures_app::updater::{Error, Target, preview};

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let result = match arguments.as_slice() {
        [flag, path] if flag == "--install" => preview::install(std::path::Path::new(&path)),
        [flag, path] if flag == "--watch-startup" => {
            preview::recovery::watch(std::path::Path::new(&path))
        }
        [flag, version, artifact, signature] if flag == "--verify" => preview::verify_package(
            &version.to_string_lossy(),
            std::path::Path::new(artifact),
            std::path::Path::new(signature),
        ),
        // Release tooling for patch updates (see updater::preview::delta).
        [flag, target, from, base, version, package, output] if flag == "--make-patch" => {
            make_patch(target, from, base, version, package, output)
        }
        #[cfg(target_os = "linux")]
        [flag, appdir, output] if flag == "--appimage" => {
            preview::appimage::build(std::path::Path::new(appdir))
                .and_then(|image| write_new(output, &image))
        }
        _ => Err(Error::Configuration(
            "usage: native_preview_update_helper --install REQUEST | --watch-startup REQUEST | --verify VERSION ARTIFACT SIGNATURE | --make-patch TARGET FROM_VERSION BASE_FOLDER VERSION PACKAGE_FOLDER OUTPUT | --appimage APPDIR OUTPUT (Linux)",
        )),
    };
    if let Err(error) = result {
        eprintln!("Captures update failed: {error}");
        std::process::exit(1);
    }
}

fn make_patch(
    target: &std::ffi::OsStr,
    from: &std::ffi::OsStr,
    base: &std::ffi::OsStr,
    version: &std::ffi::OsStr,
    package: &std::ffi::OsStr,
    output: &std::ffi::OsStr,
) -> Result<(), Error> {
    let target: Target = serde_json::from_value(serde_json::json!(target.to_string_lossy()))
        .map_err(|_| Error::Target)?;
    let parse = |text: &std::ffi::OsStr| {
        semver::Version::parse(&text.to_string_lossy()).map_err(|_| Error::Version)
    };
    let summary = preview::delta::make(
        target,
        std::path::Path::new(base),
        &parse(from)?,
        std::path::Path::new(package),
        &parse(version)?,
        std::path::Path::new(output),
    )?;
    println!(
        "{}",
        serde_json::to_string(&summary).map_err(|_| Error::Manifest)?
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn write_new(path: &std::ffi::OsStr, bytes: &[u8]) -> Result<(), Error> {
    use std::{io::Write, os::unix::fs::PermissionsExt};
    let mut file = std::fs::File::create_new(path)?;
    file.write_all(bytes)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
    Ok(file.sync_all()?)
}
