use captures_media::MediaToolchain;
use std::path::{Path, PathBuf};

/// Resolve the private media sidecars shipped beside the native executable.
/// Bare command names are retained for developers running an unpackaged build.
pub fn paths_for_executable(executable: Option<&Path>) -> (PathBuf, PathBuf) {
    let beside_executable = |name: &str| {
        executable
            .and_then(Path::parent)
            .map(|directory| directory.join(format!("{name}.exe")))
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from(name))
    };
    (beside_executable("ffmpeg"), beside_executable("ffprobe"))
}

pub fn paths() -> (PathBuf, PathBuf) {
    let executable = std::env::current_exe().ok();
    paths_for_executable(executable.as_deref())
}

pub fn toolchain() -> MediaToolchain {
    let (ffmpeg, ffprobe) = paths();
    MediaToolchain::new(ffmpeg, ffprobe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_sidecars_only_when_they_are_beside_the_executable() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("captures-windows-native.exe");
        std::fs::write(directory.path().join("ffmpeg.exe"), []).unwrap();

        let (ffmpeg, ffprobe) = paths_for_executable(Some(&executable));

        assert_eq!(ffmpeg, directory.path().join("ffmpeg.exe"));
        assert_eq!(ffprobe, PathBuf::from("ffprobe"));
    }

    #[test]
    fn uses_command_names_without_an_executable() {
        assert_eq!(
            paths_for_executable(None),
            (PathBuf::from("ffmpeg"), PathBuf::from("ffprobe"))
        );
    }
}
