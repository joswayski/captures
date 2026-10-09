//! Still-acquisition diagnostic. Creates no Captures window and never opens X11.

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The screenshot portal probe is Linux-only.");
    std::process::exit(3);
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn run() -> Result<(), String> {
    use std::{
        fs::OpenOptions,
        path::PathBuf,
        time::{Duration, Instant},
    };

    if std::env::var_os("DISPLAY").is_some() {
        return Err("DISPLAY must be unset so this probe cannot use X11.".into());
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err("WAYLAND_DISPLAY is required.".into());
    }
    let mut output = None;
    let mut timeout = Duration::from_secs(120);
    let mut cancel_after = None;
    let mut window = false;
    let mut show_cursor = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| format!("Missing value for {argument}"))?;
        match argument.as_str() {
            "--output" => output = Some(PathBuf::from(value)),
            "--target" => {
                window = match value.as_str() {
                    "display" => false,
                    "window" => true,
                    _ => return Err("Target must be display or window.".into()),
                }
            }
            "--show-cursor" => {
                show_cursor = value.parse().map_err(|_| "Cursor must be true or false")?
            }
            "--timeout-ms" | "--cancel-after-ms" => {
                let duration =
                    Duration::from_millis(value.parse().map_err(|_| "Invalid milliseconds")?);
                if argument == "--timeout-ms" {
                    timeout = duration;
                } else {
                    cancel_after = Some(duration);
                }
            }
            _ => return Err(format!("Unknown argument {argument}")),
        }
    }
    let output = output.ok_or(
        "usage: wayland_screenshot_probe --output NEW.png [--target display|window] [--show-cursor true|false] [--timeout-ms N] [--cancel-after-ms N]",
    )?;
    let started = Instant::now();
    let cancelled = || cancel_after.is_some_and(|duration| started.elapsed() >= duration);
    let image = if window {
        captures_recording_xcap::PortalVideoSource::window_screenshot(show_cursor, &cancelled)
            .map_err(|error| error.to_string())?
    } else {
        captures_capture::portal_screenshot(timeout, cancelled)
            .map_err(|error| error.to_string())?
    };
    match image {
        Some(image) => {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .map_err(|error| error.to_string())?;
            image
                .write_to(&mut file, image::ImageFormat::Png)
                .map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::json!({"event": "screenshot", "width": image.width(), "height": image.height(), "display_unset": true})
            );
        }
        None => println!(
            "{}",
            serde_json::json!({"event": "cancelled", "display_unset": true})
        ),
    }
    Ok(())
}
