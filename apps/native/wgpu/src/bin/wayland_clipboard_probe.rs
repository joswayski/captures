//! Disposable native-Wayland clipboard transport diagnostic.
//!
//! The process intentionally stays alive after publishing so an external
//! consumer can verify the image while the wgpu host-equivalent owner exists.

use std::{borrow::Cow, io::Write};

const WIDTH: usize = 3;
const HEIGHT: usize = 2;
const PIXELS: [u8; WIDTH * HEIGHT * 4] = [
    1, 2, 3, 4, 250, 17, 99, 255, 0, 127, 255, 63, 19, 211, 7, 128, 88, 44, 222, 200, 5, 6, 7, 8,
];

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() != Some("--serve-image") || arguments.next().is_some() {
        return Err("usage: wayland_clipboard_probe --serve-image".to_owned());
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err("WAYLAND_DISPLAY is required for the Wayland clipboard probe".to_owned());
    }
    if std::env::var_os("DISPLAY").is_some() {
        return Err("DISPLAY must be unset so the probe cannot fall back to X11".to_owned());
    }

    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| format!("Wayland data-control clipboard unavailable: {error}"))?;
    clipboard
        .set_image(arboard::ImageData {
            width: WIDTH,
            height: HEIGHT,
            bytes: Cow::Borrowed(&PIXELS),
        })
        .map_err(|error| format!("Could not publish the Wayland image: {error}"))?;

    println!(
        "{}",
        serde_json::json!({
            "event": "clipboard_ready",
            "width": WIDTH,
            "height": HEIGHT,
            "rgba_bytes": PIXELS.len(),
            "display_unset": true,
        })
    );
    std::io::stdout()
        .flush()
        .map_err(|error| format!("Could not flush readiness output: {error}"))?;

    // Retain both the process and Clipboard value just like the wgpu worker.
    let mut release = String::new();
    std::io::stdin()
        .read_line(&mut release)
        .map_err(|error| format!("Could not wait for clipboard verification: {error}"))?;
    drop(clipboard);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_is_asymmetric_rgba_without_transparent_color_ambiguity() {
        assert_eq!(PIXELS.len(), WIDTH * HEIGHT * 4);
        assert_eq!(&PIXELS[..4], &[1, 2, 3, 4]);
        assert_eq!(&PIXELS[PIXELS.len() - 4..], &[5, 6, 7, 8]);
        assert!(PIXELS.chunks_exact(4).all(|pixel| pixel[3] > 0));
    }
}
