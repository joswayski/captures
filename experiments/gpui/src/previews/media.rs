use crate::preferences::settings::Settings;
use anyhow::{Context, Result, bail};
use captures_media::{
    AudioEdit, CancelToken, EditSpec, ExportFormat, ExportSpec, MediaToolchain, QualityPreset,
};
use gpui::{Image, ImageFormat, RenderImage};
use image::{DynamicImage, Frame, RgbaImage, imageops};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    Image,
    Gif,
    Video,
}

#[derive(Clone)]
pub struct PreviewMedia {
    pub source: PathBuf,
    pub display: Arc<Path>,
    pub hovered: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub kind: MediaKind,
}

pub enum ClipboardContent {
    Image(Image),
    File(PathBuf),
}

impl PreviewMedia {
    pub fn load(source: PathBuf, profile: &Path) -> Result<Self> {
        let kind = media_kind(&source)?;
        let mut video_dimensions = None;
        let display = if kind == MediaKind::Video {
            let directory = profile.join("preview-posters");
            fs::create_dir_all(&directory)?;
            let poster = poster_path(&source, &directory)?;
            let tools = MediaToolchain::from_command_names();
            tools
                .verify()
                .context("video previews require FFmpeg and ffprobe")?;
            let probe = tools.probe(&source)?;
            video_dimensions = Some((probe.metadata.width, probe.metadata.height));
            if !poster.exists() {
                tools.create_poster(&source, &poster, &CancelToken::default())?;
            }
            Arc::<Path>::from(poster)
        } else {
            Arc::<Path>::from(source.clone())
        };
        let decoded = image::ImageReader::open(display.as_ref())?
            .with_guessed_format()?
            .decode()?
            .to_rgba8();
        let (width, height) = video_dimensions.unwrap_or_else(|| decoded.dimensions());
        let hovered = render_bgra(hovered_card(&decoded));
        Ok(Self {
            bytes: fs::metadata(&source)?.len(),
            source,
            display,
            hovered,
            width,
            height,
            kind,
        })
    }

    pub fn clipboard_content(&self) -> Result<ClipboardContent> {
        if self.kind == MediaKind::Video {
            return Ok(ClipboardContent::File(self.source.clone()));
        }
        let bytes = fs::read(&self.source)?;
        let format = match self.kind {
            MediaKind::Gif => ImageFormat::Gif,
            MediaKind::Image => gpui_format(&self.source)?,
            MediaKind::Video => unreachable!(),
        };
        Ok(ClipboardContent::Image(Image::from_bytes(format, bytes)))
    }
}

fn poster_path(source: &Path, directory: &Path) -> Result<PathBuf> {
    let metadata = fs::metadata(source)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let canonical = source
        .canonicalize()
        .unwrap_or_else(|_| source.to_path_buf());
    let mut identity = DefaultHasher::new();
    canonical.hash(&mut identity);
    metadata.len().hash(&mut identity);
    modified.hash(&mut identity);
    Ok(directory.join(format!("{:016x}.png", identity.finish())))
}

fn media_kind(path: &Path) -> Result<MediaKind> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "gif" => Ok(MediaKind::Gif),
        "mp4" | "mov" | "mkv" | "webm" => Ok(MediaKind::Video),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff" => Ok(MediaKind::Image),
        _ => bail!("unsupported preview file: {}", path.display()),
    }
}

fn gpui_format(path: &Path) -> Result<ImageFormat> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Ok(ImageFormat::Png),
        "jpg" | "jpeg" => Ok(ImageFormat::Jpeg),
        "webp" => Ok(ImageFormat::Webp),
        "gif" => Ok(ImageFormat::Gif),
        "bmp" => Ok(ImageFormat::Bmp),
        "tif" | "tiff" => Ok(ImageFormat::Tiff),
        _ => bail!("unsupported clipboard image: {}", path.display()),
    }
}

fn hovered_card(source: &RgbaImage) -> RgbaImage {
    let mut covered = cover_card(source, 284, 160);
    covered = imageops::blur(&covered, 2.);
    for pixel in covered.pixels_mut() {
        pixel.0[0] = ((u16::from(pixel.0[0]) * 128) / 255) as u8;
        pixel.0[1] = ((u16::from(pixel.0[1]) * 128) / 255) as u8;
        pixel.0[2] = ((u16::from(pixel.0[2]) * 128) / 255) as u8;
    }
    covered
}

pub fn cover_card(source: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    let scale = (width as f32 / source.width().max(1) as f32)
        .max(height as f32 / source.height().max(1) as f32);
    let scaled_width = (source.width() as f32 * scale).ceil() as u32;
    let scaled_height = (source.height() as f32 * scale).ceil() as u32;
    let resized = imageops::resize(
        source,
        scaled_width.max(width),
        scaled_height.max(height),
        imageops::FilterType::Triangle,
    );
    imageops::crop_imm(
        &resized,
        (resized.width() - width) / 2,
        (resized.height() - height) / 2,
        width,
        height,
    )
    .to_image()
}

pub fn render_bgra(mut rgba: RgbaImage) -> Arc<RenderImage> {
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new([Frame::new(rgba)]))
}

pub fn save(media: &PreviewMedia, settings: &Settings) -> Result<PathBuf> {
    let directory = PathBuf::from(&settings.output_directory);
    fs::create_dir_all(&directory)?;
    if media.kind == MediaKind::Gif {
        let extension = media
            .source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or(if media.kind == MediaKind::Gif {
                "gif"
            } else {
                "mp4"
            });
        let destination = unique_destination(&directory, extension);
        atomic_copy(&media.source, &destination)?;
        return Ok(destination);
    }

    if media.kind == MediaKind::Video {
        let (extension, format) = match settings.recording.video_format.as_str() {
            "gif" => ("gif", ExportFormat::Gif),
            "webm" => ("webm", ExportFormat::WebM),
            _ => ("mp4", ExportFormat::Mp4),
        };
        let destination = unique_destination(&directory, extension);
        let tools = MediaToolchain::from_command_names();
        tools.verify()?;
        if format == ExportFormat::WebM {
            export_webm(&media.source, &destination)?;
            return Ok(destination);
        }
        tools.export(
            &media.source,
            &destination,
            &EditSpec {
                trim_start_ms: 0,
                trim_end_ms: None,
                crop: None,
                output_width: None,
                output_height: None,
                audio: AudioEdit::default(),
            },
            &ExportSpec {
                format,
                quality: QualityPreset::Preserve,
                max_size_bytes: None,
                frames_per_second: (format == ExportFormat::Gif)
                    .then_some(settings.recording.gif_fps),
                gif_max_colors: (format == ExportFormat::Gif)
                    .then_some(settings.recording.gif_max_colors),
            },
            &CancelToken::default(),
            |_| {},
        )?;
        return Ok(destination);
    }

    let (extension, format) = match settings.screenshot_format.as_str() {
        "jpg" | "jpeg" => ("jpg", image::ImageFormat::Jpeg),
        "webp" => ("webp", image::ImageFormat::WebP),
        _ => ("png", image::ImageFormat::Png),
    };
    let destination = unique_destination(&directory, extension);
    if format == image::ImageFormat::Png
        && media
            .source
            .extension()
            .is_some_and(|value| value.eq_ignore_ascii_case("png"))
    {
        atomic_copy(&media.source, &destination)?;
        return Ok(destination);
    }
    let decoded = image::ImageReader::open(&media.source)?
        .with_guessed_format()?
        .decode()?;
    let mut bytes = Cursor::new(Vec::new());
    if format == image::ImageFormat::Jpeg {
        DynamicImage::ImageRgb8(decoded.to_rgb8()).write_to(&mut bytes, format)?;
    } else {
        decoded.write_to(&mut bytes, format)?;
    }
    atomic_write(&destination, bytes.get_ref())?;
    Ok(destination)
}

fn export_webm(source: &Path, destination: &Path) -> Result<()> {
    let staged = destination.with_file_name(format!(".captures-{}.webm", uuid::Uuid::new_v4()));
    let result = (|| {
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(source)
            .args(["-c:v", "libvpx-vp9", "-c:a", "libopus"])
            .arg(&staged)
            .status()?;
        if !status.success() {
            bail!("FFmpeg could not export WebM");
        }
        fs::rename(&staged, destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result
}

pub fn copy_file_to_clipboard(path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use gtk::gio::prelude::FileExt;
        // Integration already pumps GTK on the foreground thread. Let GTK own
        // the selection instead of relying on an installed clipboard process.
        if !gtk::is_initialized_main_thread() {
            bail!("native clipboard is not initialized");
        }
        let uri = gtk::gio::File::for_path(path.canonicalize()?).uri();
        let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
        if !clipboard.set_with_data(
            &[gtk::TargetEntry::new(
                "text/uri-list",
                gtk::TargetFlags::empty(),
                0,
            )],
            move |_, selection, _| {
                selection.set_uris(&[uri.as_str()]);
            },
        ) {
            bail!("could not own the file clipboard");
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "set the clipboard to POSIX file (item 1 of argv)",
            "-e",
            "end run",
            "--",
        ])
        .arg(path)
        .status()?;
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Set-Clipboard -LiteralPath $args[0]",
            "--",
        ])
        .arg(path)
        .status()?;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if !status.success() {
        bail!("could not place {} on the clipboard", path.display());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return Ok(());
}

fn unique_destination(directory: &Path, extension: &str) -> PathBuf {
    directory.join(format!("Capture-{}.{}", uuid::Uuid::new_v4(), extension))
}

fn atomic_copy(source: &Path, destination: &Path) -> Result<()> {
    let bytes = fs::read(source)?;
    atomic_write(destination, &bytes)
}

fn atomic_write(destination: &Path, bytes: &[u8]) -> Result<()> {
    let parent = destination.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    staged.write_all(bytes)?;
    staged.as_file_mut().sync_all()?;
    staged.persist(destination).map_err(|error| error.error)?;
    Ok(())
}

pub fn reveal(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .status()?;
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .status()?;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let status = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(Path::new(".")))
        .status()?;
    if !status.success() {
        bail!("could not reveal {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use std::process::Command;

    fn video_fixture(directory: &Path) -> PathBuf {
        let source = directory.join("history.mp4");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=32x24:d=0.2",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&source)
            .status()
            .expect("FFmpeg is required for preview media fixture tests");
        assert!(status.success());
        source
    }

    #[test]
    fn save_converts_to_preference_without_touching_history_png() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("history.png");
        let original = RgbaImage::from_pixel(7, 5, Rgba([21, 42, 84, 127]));
        original.save(&source).unwrap();
        let source_bytes = fs::read(&source).unwrap();
        let media = PreviewMedia::load(source.clone(), directory.path()).unwrap();
        let settings = Settings {
            output_directory: directory.path().join("exports").to_string_lossy().into(),
            screenshot_format: "jpg".into(),
            ..Settings::default()
        };
        let saved = save(&media, &settings).unwrap();
        assert_eq!(saved.extension().unwrap(), "jpg");
        assert!(image::open(saved).is_ok());
        assert_eq!(fs::read(source).unwrap(), source_bytes);
    }

    #[test]
    fn gif_save_preserves_real_gif_independently_of_video_preference() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("history.gif");
        RgbaImage::from_pixel(7, 5, Rgba([21, 42, 84, 255]))
            .save(&source)
            .unwrap();
        let source_bytes = fs::read(&source).unwrap();
        let media = PreviewMedia::load(source.clone(), directory.path()).unwrap();
        let settings = Settings {
            output_directory: directory.path().join("exports").to_string_lossy().into(),
            recording: crate::preferences::settings::RecordingSettings {
                video_format: "webm".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let saved = save(&media, &settings).unwrap();

        assert_eq!(saved.extension().unwrap(), "gif");
        assert_eq!(fs::read(saved).unwrap(), source_bytes);
        assert_eq!(fs::read(source).unwrap(), source_bytes);
    }

    #[test]
    fn video_save_exports_real_fixture_to_preferred_format_and_keeps_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = video_fixture(directory.path());
        let source_bytes = fs::read(&source).unwrap();
        let media = PreviewMedia::load(source.clone(), directory.path()).unwrap();
        let settings = Settings {
            output_directory: directory.path().join("exports").to_string_lossy().into(),
            recording: crate::preferences::settings::RecordingSettings {
                video_format: "webm".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let saved = save(&media, &settings).unwrap();

        assert_eq!(saved.extension().unwrap(), "webm");
        assert!(MediaToolchain::from_command_names().probe(&saved).is_ok());
        assert_eq!(fs::read(source).unwrap(), source_bytes);
    }

    #[test]
    fn clipboard_content_crosses_image_gif_video_boundary_without_using_video_poster() {
        let directory = tempfile::tempdir().unwrap();
        let png = directory.path().join("fixture.png");
        let gif = directory.path().join("fixture.gif");
        RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 255]))
            .save(&png)
            .unwrap();
        RgbaImage::from_pixel(2, 2, Rgba([4, 5, 6, 255]))
            .save(&gif)
            .unwrap();
        let video = video_fixture(directory.path());

        assert!(matches!(
            PreviewMedia::load(png, directory.path())
                .unwrap()
                .clipboard_content()
                .unwrap(),
            ClipboardContent::Image(_)
        ));
        assert!(matches!(
            PreviewMedia::load(gif, directory.path())
                .unwrap()
                .clipboard_content()
                .unwrap(),
            ClipboardContent::Image(_)
        ));
        match PreviewMedia::load(video.clone(), directory.path())
            .unwrap()
            .clipboard_content()
            .unwrap()
        {
            ClipboardContent::File(path) => assert_eq!(path, video),
            ClipboardContent::Image(_) => {
                panic!("video copy must use the media file, not its poster")
            }
        }
    }

    #[test]
    fn cover_crop_is_centered_for_asymmetric_portrait_and_landscape_sources() {
        let landscape = RgbaImage::from_fn(1000, 200, |x, _| Rgba([(x / 4) as u8, 0, 0, 255]));
        let portrait = RgbaImage::from_fn(200, 1000, |_, y| Rgba([0, (y / 4) as u8, 0, 255]));
        let a = cover_card(&landscape, 284, 160);
        let b = cover_card(&portrait, 284, 160);
        assert_eq!(a.dimensions(), (284, 160));
        assert_eq!(b.dimensions(), (284, 160));
        assert!(a.get_pixel(0, 80)[0] > 30);
        assert!(b.get_pixel(142, 0)[1] > 30);
    }

    #[test]
    fn video_poster_path_is_stable_per_source_identity() {
        let directory = tempfile::tempdir().unwrap();
        let source_a = directory.path().join("a.mp4");
        let source_b = directory.path().join("b.mp4");
        fs::write(&source_a, b"video-a").unwrap();
        fs::write(&source_b, b"video-b-longer").unwrap();
        let posters = directory.path().join("posters");

        let first = poster_path(&source_a, &posters).unwrap();
        let second = poster_path(&source_a, &posters).unwrap();
        let other = poster_path(&source_b, &posters).unwrap();

        assert_eq!(first, second);
        assert_ne!(first, other);
        assert_eq!(first.parent(), Some(posters.as_path()));
    }
}
