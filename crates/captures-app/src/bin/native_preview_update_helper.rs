//! Installed Preview helper, copied outside the package before the GUI exits.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

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
        _ => Err(captures_app::updater::Error::Configuration(
            "usage: native_preview_update_helper --install REQUEST | --watch-startup REQUEST | --verify VERSION ARTIFACT SIGNATURE",
        )),
    };
    if let Err(error) = result {
        eprintln!("Captures Preview update failed: {error}");
        std::process::exit(1);
    }
}
