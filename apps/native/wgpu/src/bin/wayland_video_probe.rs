//! No-window ScreenCast/PipeWire diagnostic. Does not enable native recording UI.
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The video portal probe is Linux-only.");
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
    use captures_recording::RecordingTarget;
    use captures_recording_xcap::PortalVideoSource;
    use std::{
        collections::BTreeSet,
        fs::OpenOptions,
        path::PathBuf,
        sync::mpsc::RecvTimeoutError,
        time::{Duration, Instant},
    };

    if std::env::var_os("DISPLAY").is_some() {
        return Err("DISPLAY must be unset so this probe cannot use X11.".into());
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err("WAYLAND_DISPLAY is required.".into());
    }
    let mut output = None;
    let mut count = 3_u32;
    let mut cancel_after = None;
    let mut show_cursor = false;
    let mut target = RecordingTarget::PortalDisplay;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| format!("Missing value for {argument}"))?;
        match argument.as_str() {
            "--output" => output = Some(PathBuf::from(value)),
            "--frames" => count = value.parse().map_err(|_| "Invalid frame count")?,
            "--cancel-after-ms" => {
                cancel_after = Some(Duration::from_millis(
                    value.parse().map_err(|_| "Invalid milliseconds")?,
                ))
            }
            "--show-cursor" => {
                show_cursor = value.parse().map_err(|_| "Cursor must be true or false")?
            }
            "--target" => {
                target = match value.as_str() {
                    "display" => RecordingTarget::PortalDisplay,
                    "window" => RecordingTarget::PortalWindow,
                    _ => return Err("Target must be display or window.".into()),
                }
            }
            _ => return Err(format!("Unknown argument {argument}")),
        }
    }
    if !(1..=120).contains(&count) {
        return Err("Frame count must be between 1 and 120.".into());
    }
    let output = output.ok_or("usage: wayland_video_probe --output NEW.png [--frames N] [--cancel-after-ms N] [--show-cursor true|false]")?;
    let started = Instant::now();
    let cancelled = || cancel_after.is_some_and(|duration| started.elapsed() >= duration);
    let (source, frames) = PortalVideoSource::start(&target, show_cursor, 30, &cancelled)
        .map_err(|error| error.to_string())?;
    let mut last = None;
    let mut corner_colors = BTreeSet::new();
    let mut deadline = Instant::now() + Duration::from_secs(5);
    for collected in 0..count {
        loop {
            if cancelled() {
                return Err("Video portal diagnostic cancelled.".into());
            }
            if Instant::now() >= deadline {
                return Err(source.warning().unwrap_or_else(|| {
                    format!("Timed out after {collected} portal video frames.")
                }));
            }
            match frames.recv_timeout(Duration::from_millis(50)) {
                Ok(frame) => {
                    let frame = image::RgbaImage::from_raw(frame.width, frame.height, frame.raw)
                        .ok_or("Invalid portal RGBA frame")?;
                    corner_colors.insert(frame.get_pixel(0, 0).0);
                    last = Some(frame);
                    deadline = Instant::now() + Duration::from_secs(5);
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(source
                        .warning()
                        .unwrap_or_else(|| "Portal video stream ended.".into()));
                }
            }
        }
    }
    let warning = source.warning();
    let dropped = source.dropped_frames();
    source.stop()?;
    if let Some(warning) = warning {
        return Err(warning);
    }
    let image = last.ok_or("No portal video frame.")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .map_err(|error| error.to_string())?;
    if let Err(error) = image.write_to(&mut file, image::ImageFormat::Png) {
        drop(file);
        let _ = std::fs::remove_file(&output);
        return Err(error.to_string());
    }
    println!(
        "{}",
        serde_json::json!({"event":"video", "frames":count, "width":image.width(), "height":image.height(), "dropped_frames":dropped, "corner_colors":corner_colors, "display_unset":true})
    );
    Ok(())
}
