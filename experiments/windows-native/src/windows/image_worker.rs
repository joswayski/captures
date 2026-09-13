use captures_windows_native::{
    async_state::LatestQueue, editor::Document, encoder, state::EditorQualityMode,
};
use image::RgbaImage;
use std::{
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, Sender, channel},
    },
    thread,
};

#[derive(Clone)]
pub struct EncodeSpec {
    pub format: String,
    pub quality_mode: EditorQualityMode,
    pub quality: u8,
    pub maximum_bytes: u64,
    pub width: u32,
    pub height: u32,
}

pub enum Event {
    Preview {
        key: (u64, u64),
        result: Result<RgbaImage, String>,
    },
    Estimate {
        key: (u64, u64),
        request: u64,
        result: Result<u64, String>,
    },
    Copy {
        document_id: u64,
        request: u64,
        result: Result<RgbaImage, String>,
    },
    Save {
        document_key: (u64, u64),
        request: u64,
        destination: PathBuf,
        dimensions: (u32, u32),
        result: Result<(), String>,
    },
}

pub struct ImageWorker {
    sender: Sender<Event>,
    receiver: Receiver<Event>,
    estimates: Arc<LatestQueue<EstimateJob>>,
}

struct EstimateJob {
    key: (u64, u64),
    document: Document,
    spec: EncodeSpec,
}

impl ImageWorker {
    pub fn new() -> Self {
        let (sender, receiver) = channel();
        let estimates = Arc::new(LatestQueue::<EstimateJob>::new());
        let estimate_queue = estimates.clone();
        let estimate_sender = sender.clone();
        thread::spawn(move || {
            while let Some((request, job)) = estimate_queue.take() {
                let result = render_and_encode(&job.document, &job.spec)
                    .map(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                if estimate_queue.is_latest(request) {
                    let _ = estimate_sender.send(Event::Estimate {
                        key: job.key,
                        request,
                        result,
                    });
                }
            }
        });
        Self {
            sender,
            receiver,
            estimates,
        }
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.receiver.try_recv().ok()
    }

    pub fn render_preview(&self, key: (u64, u64), document: Document) {
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = document.render();
            let _ = sender.send(Event::Preview { key, result });
        });
    }

    pub fn estimate(&self, key: (u64, u64), request: u64, document: Document, spec: EncodeSpec) {
        self.estimates.submit(
            request,
            EstimateJob {
                key,
                document,
                spec,
            },
        );
    }

    pub fn copy(&self, request: u64, document: Document, dimensions: (u32, u32)) {
        let document_id = document.render_key().0;
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = document.render_resized(dimensions.0, dimensions.1);
            let _ = sender.send(Event::Copy {
                document_id,
                request,
                result,
            });
        });
    }

    pub fn save(
        &self,
        request: u64,
        document: Document,
        spec: EncodeSpec,
        destination: PathBuf,
        replace: Option<PathBuf>,
    ) {
        let document_key = document.render_key();
        let sender = self.sender.clone();
        thread::spawn(move || {
            let dimensions = (spec.width, spec.height);
            let reported_destination = replace.clone().unwrap_or_else(|| destination.clone());
            let result = render_and_encode(&document, &spec).and_then(|bytes| {
                if let Some(original) = replace {
                    write_replacement(&destination, &original, &bytes)
                } else {
                    write_new(&destination, &bytes)
                }
            });
            let _ = sender.send(Event::Save {
                document_key,
                request,
                destination: reported_destination,
                dimensions,
                result,
            });
        });
    }
}

impl Drop for ImageWorker {
    fn drop(&mut self) {
        self.estimates.close();
    }
}

fn render_and_encode(document: &Document, spec: &EncodeSpec) -> Result<Vec<u8>, String> {
    let image = document.render_resized(spec.width, spec.height)?;
    match (spec.format.as_str(), spec.quality_mode) {
        ("jpeg", EditorQualityMode::Preserve) => encoder::encode_jpeg(&image, 100),
        ("jpeg", EditorQualityMode::Compress) => encoder::encode_jpeg(&image, spec.quality),
        ("jpeg", EditorQualityMode::Maximum) => {
            encoder::encode_jpeg_with_limit(&image, spec.maximum_bytes)
        }
        ("webp", EditorQualityMode::Preserve) => encoder::encode_webp(&image, None),
        ("webp", EditorQualityMode::Compress) => encoder::encode_webp(&image, Some(spec.quality)),
        ("webp", EditorQualityMode::Maximum) => {
            encoder::encode_webp_with_limit(&image, spec.maximum_bytes)
        }
        (_, EditorQualityMode::Preserve) => encoder::encode_png(&image, None),
        (_, EditorQualityMode::Compress) => encoder::encode_png(&image, Some(spec.quality)),
        (_, EditorQualityMode::Maximum) => {
            encoder::encode_png_with_limit(&image, spec.maximum_bytes)
        }
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    let result = (|| {
        let mut writer = BufWriter::new(file);
        writer.write_all(bytes).map_err(|error| error.to_string())?;
        writer.flush().map_err(|error| error.to_string())?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

fn write_replacement(staged: &Path, original: &Path, bytes: &[u8]) -> Result<(), String> {
    write_new(staged, bytes)?;
    let backup = original.with_file_name(format!(
        ".{}.captures-backup-{}",
        original
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("image"),
        uuid::Uuid::new_v4()
    ));
    if let Err(error) = fs::rename(original, &backup) {
        let _ = fs::remove_file(staged);
        return Err(error.to_string());
    }
    if let Err(error) = fs::rename(staged, original) {
        let rollback = fs::rename(&backup, original);
        return Err(match rollback {
            Ok(()) => format!("could not replace image: {error}"),
            Err(rollback) => format!(
                "could not replace image ({error}) or restore backup {} ({rollback})",
                backup.display()
            ),
        });
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, Rgba};

    #[test]
    fn selected_encode_uses_requested_dimensions_and_format() {
        let image = RgbaImage::from_fn(37, 19, |x, y| {
            Rgba([(x * 7) as u8, (y * 11) as u8, (x + y) as u8, 255])
        });
        let document = Document::new(image);
        let spec = EncodeSpec {
            format: "jpeg".into(),
            quality_mode: EditorQualityMode::Compress,
            quality: 70,
            maximum_bytes: 50_000,
            width: 23,
            height: 41,
        };
        let bytes = render_and_encode(&document, &spec).unwrap();
        assert_eq!(&bytes[..2], &[0xff, 0xd8]);
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!(decoded.dimensions(), (23, 41));
    }

    #[test]
    fn replacement_writes_only_job_owned_stage_and_requested_source() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.png");
        let unrelated = directory.path().join("keep.png");
        let staged = directory.path().join("stage.png");
        fs::write(&original, b"old").unwrap();
        fs::write(&unrelated, b"keep").unwrap();
        write_replacement(&staged, &original, b"new").unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"new");
        assert_eq!(fs::read(&unrelated).unwrap(), b"keep");
        assert!(!staged.exists());
    }
}
