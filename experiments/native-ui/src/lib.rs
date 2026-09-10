//! Shared workload; neither frontend changes the production app's settings/history.
use std::{io::Cursor, path::PathBuf, sync::Mutex, time::Instant};

use captures_capture::XcapBackend;
use image::ImageFormat;
use serde::Serialize;

#[derive(Default)]
pub struct CaptureStore(Mutex<Option<Vec<u8>>>);

#[derive(Serialize)]
pub struct Capture {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub capture_encode_ms: f64,
}

impl CaptureStore {
    pub fn capture(&self) -> Result<Capture, String> {
        let start = Instant::now();
        let backend = XcapBackend;
        backend.ensure_permission(true).map_err(|e| e.to_string())?;
        let frame = backend
            .capture_display_at_point(None)
            .map_err(|e| e.to_string())?;
        let (width, height) = frame.image.dimensions();
        let mut png = Cursor::new(Vec::new());
        frame
            .image
            .write_to(&mut png, ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        let png = png.into_inner();
        let capture_encode_ms = start.elapsed().as_secs_f64() * 1000.0;
        *self.0.lock().map_err(|e| e.to_string())? = Some(png.clone());
        Ok(Capture {
            png,
            width,
            height,
            capture_encode_ms,
        })
    }

    pub fn save(&self) -> Result<String, String> {
        let directory = std::env::var_os("CAPTURES_PROBE_OUTPUT")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("captures-ui-probe"));
        self.save_in(&directory)
    }

    fn save_in(&self, directory: &std::path::Path) -> Result<String, String> {
        let guard = self.0.lock().map_err(|e| e.to_string())?;
        let png = guard.as_ref().ok_or("Capture a display first")?;
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        // Never overwrite an existing capture, even across repeated probe launches.
        for index in 1.. {
            let path = directory.join(format!("capture-{index}.png"));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    use std::io::Write;
                    file.write_all(png).map_err(|e| e.to_string())?;
                    return Ok(format!("Saved {}", path.display()));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        unreachable!()
    }
}

/// A probe-only readiness signal. Call after scheduling the initial UI draw.
pub fn ready() -> Result<(), String> {
    if let Some(path) = std::env::var_os("CAPTURES_PROBE_READY") {
        std::fs::write(path, "ready").map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_without_capture_does_not_create_a_file() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("not-created");
        assert_eq!(
            CaptureStore::default().save_in(&output).unwrap_err(),
            "Capture a display first"
        );
        assert!(!output.exists());
    }

    #[test]
    fn saving_preserves_existing_files_and_exact_capture_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let existing = temp.path().join("capture-1.png");
        std::fs::write(&existing, b"existing capture").unwrap();
        let store = CaptureStore(Mutex::new(Some(vec![1, 9, 3, 7])));
        store.save_in(temp.path()).unwrap();
        store.save_in(temp.path()).unwrap();
        assert_eq!(std::fs::read(existing).unwrap(), b"existing capture");
        for name in ["capture-2.png", "capture-3.png"] {
            assert_eq!(std::fs::read(temp.path().join(name)).unwrap(), [1, 9, 3, 7]);
        }
    }
}
