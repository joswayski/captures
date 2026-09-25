//! Show a saved file in the desktop file manager, matching the shipping
//! `reveal_item_in_dir`: select the file where the platform supports it and
//! otherwise open its folder.

use std::{path::Path, process::Command};

pub fn reveal(path: &Path) -> std::io::Result<()> {
    if !path.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("saved file no longer exists: {}", path.display()),
        ));
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Relative paths cannot form a file:// URI; the saved path is absolute
        // in practice, but never hand a file manager a half-formed request.
        let absolute = std::path::absolute(path)?;
        #[cfg(target_os = "linux")]
        if show_items(&absolute).is_ok() {
            return Ok(());
        }
        Command::new("xdg-open")
            .arg(absolute.parent().unwrap_or(&absolute))
            .spawn()
            .map(|_| ())
    }
}

/// org.freedesktop.FileManager1.ShowItems selects the file in Files, Dolphin,
/// Nemo, Thunar, Caja and other implementers.
#[cfg(target_os = "linux")]
fn show_items(path: &Path) -> Result<(), dbus::Error> {
    use std::time::Duration;

    let connection = dbus::blocking::Connection::new_session()?;
    // Generous enough for D-Bus activation of a cold file manager; missing
    // services fail immediately and fall back to opening the folder.
    let proxy = connection.with_proxy(
        "org.freedesktop.FileManager1",
        "/org/freedesktop/FileManager1",
        Duration::from_secs(5),
    );
    proxy.method_call(
        "org.freedesktop.FileManager1",
        "ShowItems",
        (vec![file_uri(path)], ""),
    )
}

/// RFC 8089 file URI: keep unreserved bytes and `/`, percent-encode the rest
/// of the raw path bytes (spaces, `#`, `%`, non-UTF-8 names).
#[cfg(unix)]
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;

    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn file_uri_percent_encodes_raw_path_bytes() {
        assert_eq!(
            file_uri(Path::new("/home/me/Pictures/Screenshot 1.png")),
            "file:///home/me/Pictures/Screenshot%201.png"
        );
        assert_eq!(
            file_uri(Path::new("/tmp/ü#100%?.png")),
            "file:///tmp/%C3%BC%23100%25%3F.png"
        );
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(
            file_uri(Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff"))),
            "file:///tmp/%FF"
        );
    }

    #[test]
    fn missing_file_is_reported_without_launching_anything() {
        let error = reveal(Path::new("/definitely/missing/capture.png")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }
}
