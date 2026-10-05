//! Native development packages share Tauri's already-built media sidecars.
use std::path::{Path, PathBuf};

use captures_media::MediaToolchain;

pub fn locate() -> MediaToolchain {
    let executable = std::env::current_exe().ok();
    let target = format!(
        "{}-{}",
        std::env::consts::ARCH,
        match std::env::consts::OS {
            "macos" => "apple-darwin",
            "windows" => "pc-windows-msvc",
            _ => "unknown-linux-gnu",
        }
    );
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../desktop/src-tauri/binaries");
    let tool = |name, variable| {
        locate_tool(
            name,
            &target,
            executable.as_deref(),
            std::env::var_os(variable).as_deref().map(Path::new),
            &source,
        )
    };
    MediaToolchain::new(
        tool("ffmpeg", "CAPTURES_FFMPEG"),
        tool("ffprobe", "CAPTURES_FFPROBE"),
    )
}

/// The exact pair installed in a native package. Update health must not accept
/// development overrides, checkout sidecars, siblings, or commands from PATH.
pub fn verify_bundled() -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the packaged executable: {error}"))?;
    verify_bundled_at(&executable, &target()).map_err(|error| error.to_string())
}

fn target() -> String {
    format!(
        "{}-{}",
        std::env::consts::ARCH,
        match std::env::consts::OS {
            "macos" => "apple-darwin",
            "windows" => "pc-windows-msvc",
            _ => "unknown-linux-gnu",
        }
    )
}

fn verify_bundled_at(
    executable: &Path,
    target: &str,
) -> Result<(), captures_media::MediaToolError> {
    let suffix = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let binaries = executable
        .parent()
        .unwrap_or(Path::new(""))
        .join("binaries");
    MediaToolchain::new(
        binaries.join(format!("ffmpeg-{target}{suffix}")),
        binaries.join(format!("ffprobe-{target}{suffix}")),
    )
    .verify()
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn locate_tool(
    name: &str,
    target: &str,
    executable: Option<&Path>,
    override_path: Option<&Path>,
    source: &Path,
) -> PathBuf {
    if let Some(path) = override_path
        .and_then(|path| std::path::absolute(path).ok())
        .filter(|path| executable_file(path))
    {
        return path;
    }
    let filename = format!(
        "{name}-{target}{}",
        if target.contains("windows") {
            ".exe"
        } else {
            ""
        }
    );
    let parent = executable.and_then(Path::parent);
    parent
        .into_iter()
        .flat_map(|parent| {
            [
                parent.join(&filename),
                parent.join("binaries").join(&filename),
            ]
        })
        .chain([source.join(&filename)])
        .find(|path| executable_file(path))
        .unwrap_or_else(|| PathBuf::from(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"fixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn bundled_pair_wins_over_checkout_and_preserves_platform_filenames() {
        for (target, ffmpeg, ffprobe) in [
            (
                "aarch64-apple-darwin",
                "ffmpeg-aarch64-apple-darwin",
                "ffprobe-aarch64-apple-darwin",
            ),
            (
                "x86_64-apple-darwin",
                "ffmpeg-x86_64-apple-darwin",
                "ffprobe-x86_64-apple-darwin",
            ),
            (
                "x86_64-pc-windows-msvc",
                "ffmpeg-x86_64-pc-windows-msvc.exe",
                "ffprobe-x86_64-pc-windows-msvc.exe",
            ),
            (
                "x86_64-unknown-linux-gnu",
                "ffmpeg-x86_64-unknown-linux-gnu",
                "ffprobe-x86_64-unknown-linux-gnu",
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let executable = root.path().join("é package/app");
            let source = root.path().join("checkout");
            for (name, filename) in [("ffmpeg", ffmpeg), ("ffprobe", ffprobe)] {
                let bundled = executable.parent().unwrap().join("binaries").join(filename);
                tool(&bundled);
                tool(&source.join(filename));
                assert_eq!(
                    locate_tool(name, target, Some(&executable), None, &source),
                    bundled
                );
                // A sibling takes precedence over the nested bundle.
                let sibling = executable.parent().unwrap().join(filename);
                tool(&sibling);
                assert_eq!(
                    locate_tool(name, target, Some(&executable), None, &source),
                    sibling
                );
            }
        }
    }

    #[test]
    fn overrides_checkout_and_command_fallback_match_appkit() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("checkout");
        let explicit = root.path().join("override é");
        let target = "x86_64-unknown-linux-gnu";
        assert_eq!(
            locate_tool("ffmpeg", target, None, Some(&explicit), &source),
            PathBuf::from("ffmpeg")
        );
        let checkout = source.join("ffmpeg-x86_64-unknown-linux-gnu");
        tool(&checkout);
        assert_eq!(
            locate_tool("ffmpeg", target, None, Some(&explicit), &source),
            checkout
        );
        tool(&explicit);
        assert_eq!(
            locate_tool("ffmpeg", target, None, Some(&explicit), &source),
            explicit
        );
        assert_eq!(
            locate_tool("ffprobe", target, None, None, &source),
            PathBuf::from("ffprobe")
        );
    }

    #[test]
    fn bare_relative_override_is_an_absolute_path_not_a_path_search() {
        let file = tempfile::NamedTempFile::new_in(std::env::current_dir().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let relative = Path::new(file.path().file_name().unwrap());
        assert_eq!(
            locate_tool(
                "ffmpeg",
                "x86_64-unknown-linux-gnu",
                None,
                Some(relative),
                file.path()
            ),
            file.path()
        );
    }

    #[test]
    #[cfg(unix)]
    fn nonexecutable_override_is_not_selected() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let invalid = root.path().join("not executable");
        tool(&invalid);
        std::fs::set_permissions(&invalid, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            locate_tool(
                "ffmpeg",
                "x86_64-unknown-linux-gnu",
                None,
                Some(&invalid),
                root.path()
            ),
            PathBuf::from("ffmpeg")
        );
    }

    #[test]
    #[cfg(unix)]
    fn health_verification_uses_only_the_nested_packaged_pair() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("Captures");
        let target = "x86_64-unknown-linux-gnu";
        for name in ["ffmpeg", "ffprobe"] {
            let packaged = root
                .path()
                .join("binaries")
                .join(format!("{name}-{target}"));
            std::fs::create_dir_all(packaged.parent().unwrap()).unwrap();
            std::fs::write(&packaged, b"#!/bin/sh\nexit 0\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&packaged, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(verify_bundled_at(&executable, target).is_ok());
        std::fs::remove_file(
            root.path()
                .join("binaries")
                .join(format!("ffmpeg-{target}")),
        )
        .unwrap();
        let sibling = root.path().join(format!("ffmpeg-{target}"));
        tool(&sibling);
        assert!(
            verify_bundled_at(&executable, target).is_err(),
            "a sibling fallback must not satisfy update health"
        );
    }
}
