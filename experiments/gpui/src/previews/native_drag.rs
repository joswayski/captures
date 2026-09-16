//! Outbound Copy-only file dragging for macOS, Windows and X11.
//! Native Wayland requires a source surface and input serial from GPUI's backend.

use anyhow::{Context as _, Result, bail};
use gpui::{App, AsyncApp, Render, Timer, WindowHandle};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::mpsc::{TryRecvError, sync_channel},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeDragOutcome {
    Dropped,
    Cancelled,
}

pub fn supported() -> bool {
    !cfg!(target_os = "linux") || std::env::var_os("WAYLAND_DISPLAY").is_none()
}

/// Replace the just-installed GPUI drag with an operating-system file drag.
///
/// This must be called from the GPUI thread in an `on_drag` callback. The
/// one-tick deferral lets GPUI finish installing its drag first; the helper
/// then clears that drag before entering the platform drag loop. Completion is
/// posted back to the GPUI thread, including cancellation and startup errors.
pub fn start_native_file_drag<V, F>(
    window: WindowHandle<V>,
    paths: Vec<PathBuf>,
    poster: PathBuf,
    cx: &mut App,
    completion: F,
) where
    V: Render + 'static,
    F: FnOnce(Result<NativeDragOutcome>, &mut App) + 'static,
{
    let prepared = prepare(paths, poster);
    cx.spawn(async move |cx| {
        let completion = move |result, cx: &mut AsyncApp| {
            let _ = cx.update(move |cx| completion(result, cx));
        };
        // `on_drag` has not installed `App::active_drag` until its callback
        // returns, so starting native DnD inline would leave the ghost alive.
        Timer::after(Duration::from_millis(1)).await;

        if let Err(error) = window.update(cx, |_, gpui_window, app| {
            app.stop_active_drag(gpui_window);
        }) {
            completion(
                Err(error).context("preview window closed before native drag started"),
                cx,
            );
            return;
        }
        let (paths, poster) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                completion(Err(error), cx);
                return;
            }
        };
        let (sender, receiver) = sync_channel::<Result<NativeDragOutcome>>(1);
        let start: Result<()> = window
            .update(cx, move |_, gpui_window, _| {
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                {
                    drag::start_drag(
                        gpui_window,
                        drag::DragItem::Files(paths),
                        drag::Image::Raw(poster),
                        move |result, _| {
                            // Bounded and non-blocking: a native operation has one
                            // terminal result, and the GPUI task owns the receiver.
                            let _ = sender.try_send(Ok(match result {
                                drag::DragResult::Dropped => NativeDragOutcome::Dropped,
                                drag::DragResult::Cancel => NativeDragOutcome::Cancelled,
                            }));
                        },
                        drag::Options {
                            mode: drag::DragMode::Copy,
                            ..Default::default()
                        },
                    )
                    .context("failed to start native file drag")
                }
                #[cfg(target_os = "linux")]
                {
                    let _ = gpui_window;
                    std::thread::Builder::new()
                        .name("capture-file-drag".into())
                        .spawn(move || {
                            let result = super::xdnd::run(paths, poster).map(|dropped| {
                                if dropped {
                                    NativeDragOutcome::Dropped
                                } else {
                                    NativeDragOutcome::Cancelled
                                }
                            });
                            let _ = sender.try_send(result);
                        })
                        .context("could not start X11 file drag worker")?;
                    Ok(())
                }
            })
            .context("preview window closed before native drag started")
            .and_then(|result| result);

        if let Err(error) = start {
            completion(Err(error), cx);
            return;
        }
        loop {
            match receiver.try_recv() {
                Ok(result) => {
                    completion(result, cx);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    completion(
                        Err(anyhow::anyhow!("native drag ended without a result")),
                        cx,
                    );
                    return;
                }
                Err(TryRecvError::Empty) => {
                    Timer::after(Duration::from_millis(16)).await;
                }
            }
        }
    })
    .detach();
}

fn prepare(paths: Vec<PathBuf>, poster: PathBuf) -> Result<(Vec<PathBuf>, Vec<u8>)> {
    if paths.is_empty() {
        bail!("native file drag requires at least one file");
    }
    let paths = paths
        .into_iter()
        .map(|path| canonical_file(&path))
        .collect::<Result<Vec<_>>>()?;
    let poster = canonical_file(&poster).context("invalid native drag poster path")?;
    // Encode a small known-valid PNG; a decoder accepting e.g. WebP does not
    // establish that NSImage supports the original file format on this OS.
    let image = image::open(&poster).with_context(|| {
        format!(
            "native drag poster is not a valid image: {}",
            poster.display()
        )
    })?;
    let card = super::media::cover_card(&image.to_rgba8(), 284, 162);
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(card).write_to(&mut png, image::ImageFormat::Png)?;
    Ok((paths, png.into_inner()))
}

fn canonical_file(path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("native drag file does not exist: {}", path.display()))?;
    if !path.is_file() {
        bail!("native drag path is not a file: {}", path.display());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_payload_is_canonical_and_icon_is_png_without_changing_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("capture #1.webp");
        image::RgbaImage::from_pixel(480, 210, image::Rgba([17, 93, 141, 255]))
            .save(&source)
            .unwrap();
        let original = std::fs::read(&source).unwrap();
        let (paths, bytes) = prepare(vec![source.clone()], source.clone()).unwrap();
        assert_eq!(paths, [source.canonicalize().unwrap()]);
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let icon = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(icon.dimensions(), (284, 162));
        assert_eq!(icon.get_pixel(142, 81).0, [17, 93, 141, 255]);
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert!(prepare(vec![directory.path().to_owned()], source.clone()).is_err());
        let corrupt = directory.path().join("corrupt.png");
        std::fs::write(&corrupt, b"not an image").unwrap();
        assert!(prepare(vec![source], corrupt).is_err());
    }
}
