//! Installed Preview helper, copied outside the package before the GUI exits.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use captures_app::updater::preview::delta;

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let result = match arguments.as_slice() {
        [flag, path] if flag == "--install" => {
            captures_app::updater::preview::install(std::path::Path::new(&path))
        }
        [flag, path] if flag == "--watch-startup" => {
            captures_app::updater::preview::recovery::watch(std::path::Path::new(&path))
        }
        [flag, version, artifact, signature] if flag == "--verify" => {
            captures_app::updater::preview::verify_package(
                &version.to_string_lossy(),
                std::path::Path::new(artifact),
                std::path::Path::new(signature),
            )
        }
        // Release tooling for patch updates (see updater::preview::delta).
        [flag, app, output] if flag == "--bundle-tar" => {
            delta::bundle_tar(std::path::Path::new(app)).and_then(|tar| write_new(output, &tar))
        }
        [flag, base, target, output] if flag == "--make-patch" => std::fs::read(base)
            .and_then(|base| Ok((base, std::fs::read(target)?)))
            .map_err(Into::into)
            .and_then(|(base, target)| delta::make(&base, &target))
            .and_then(|patch| write_new(output, &patch)),
        _ => Err(captures_app::updater::Error::Configuration(
            "usage: native_preview_update_helper --install REQUEST | --watch-startup REQUEST | --verify VERSION ARTIFACT SIGNATURE | --bundle-tar APP OUTPUT | --make-patch BASE TARGET OUTPUT",
        )),
    };
    if let Err(error) = result {
        eprintln!("Captures update failed: {error}");
        std::process::exit(1);
    }
}

fn write_new(path: &std::ffi::OsStr, bytes: &[u8]) -> Result<(), captures_app::updater::Error> {
    use std::io::Write;
    let mut file = std::fs::File::create_new(path)?;
    file.write_all(bytes)?;
    Ok(file.sync_all()?)
}
