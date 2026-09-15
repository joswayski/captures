//! Media tools resolve without depending on a terminal's environment.
use std::path::{Path, PathBuf};

pub fn tool(name: &str) -> PathBuf {
    resolve(
        name,
        std::env::current_exe().ok().as_deref(),
        cfg!(target_os = "macos"),
        cfg!(windows),
    )
}

fn resolve(name: &str, executable: Option<&Path>, macos: bool, windows: bool) -> PathBuf {
    let filename = if windows {
        format!("{name}.exe")
    } else {
        name.into()
    };
    if let Some(directory) = executable.and_then(Path::parent) {
        let bundled = if macos {
            directory.join("../Resources/bin").join(&filename)
        } else {
            directory.join(&filename)
        };
        if bundled.is_file() {
            return bundled;
        }
    }
    // Finder does not inherit a shell's Homebrew PATH. Do not change the process
    // environment: subprocesses and unrelated libraries retain their own PATH.
    if macos {
        for directory in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
            let installed = Path::new(directory).join(&filename);
            if installed.is_file() {
                return installed;
            }
        }
    }
    filename.into()
}

pub fn toolchain() -> captures_media::MediaToolchain {
    captures_media::MediaToolchain::new(tool("ffmpeg"), tool("ffprobe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_platform_bundle_layout_without_using_working_directory() {
        let root = tempfile::tempdir().unwrap();
        let binary = root
            .path()
            .join("Captures.app/Contents/MacOS/captures-gpui");
        let resources = root.path().join("Captures.app/Contents/Resources/bin");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(resources.join("ffmpeg"), b"bundled").unwrap();
        assert_eq!(
            resolve("ffmpeg", Some(&binary), true, false)
                .canonicalize()
                .unwrap(),
            resources.join("ffmpeg").canonicalize().unwrap()
        );
        let sibling = binary.parent().unwrap().join("ffmpeg.exe");
        std::fs::write(&sibling, b"windows").unwrap();
        assert_eq!(resolve("ffmpeg", Some(&binary), false, true), sibling);
        // A Mac resource and a Windows .exe must not be used for Linux.
        assert_eq!(
            resolve("ffmpeg", Some(&binary), false, false),
            PathBuf::from("ffmpeg")
        );
        let linux = binary.parent().unwrap().join("ffmpeg");
        std::fs::write(&linux, b"linux").unwrap();
        assert_eq!(resolve("ffmpeg", Some(&binary), false, false), linux);
        assert_eq!(
            resolve("ffprobe", None, false, true),
            PathBuf::from("ffprobe.exe")
        );
    }
}
