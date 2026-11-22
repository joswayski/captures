//! Display stills on Windows, through GDI: about 65 ms at 4K whether or not
//! the desktop is changing, with no Direct3D device. xcap's
//! Windows.Graphics.Capture (WGC) grab left about 32 MB of GPU memory charged
//! to the process per grab at 4K on NVIDIA, never reclaimed, and any per-grab
//! WGC session does. DXGI desktop duplication gets no frame from a still
//! desktop, the usual case, so it waited 250 ms and fell back to GDI anyway.
//! GDI returns the same pixels as duplication.

use image::RgbaImage;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDC, ReleaseDC, SRCCOPY, SelectObject,
};

use crate::{CaptureError, CaptureResult};

/// The desktop at physical rect `(x, y, width, height)`, without the cursor.
pub(crate) fn capture_display(x: i32, y: i32, width: u32, height: u32) -> CaptureResult<RgbaImage> {
    let failed = |what: &str| CaptureError::Backend(format!("GDI screen grab: {what}"));
    let (w, h) = (
        i32::try_from(width).map_err(|_| failed("width"))?,
        i32::try_from(height).map_err(|_| failed("height"))?,
    );
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(failed("GetDC"));
        }
        let memory = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let result = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .map_err(|error| failed(&error.to_string()))
            .and_then(|bitmap| {
                let previous = SelectObject(memory, bitmap.into());
                let copied = BitBlt(memory, 0, 0, w, h, Some(screen), x, y, SRCCOPY);
                let _ = GdiFlush();
                let image = match copied {
                    Ok(()) if !bits.is_null() => {
                        // A top-down 32 bpp DIB: rows are `width * 4` bytes apart.
                        let bgra = std::slice::from_raw_parts(
                            bits.cast::<u8>().cast_const(),
                            width as usize * height as usize * 4,
                        );
                        let mut rgba = vec![0u8; bgra.len()];
                        bgra_to_opaque_rgba(bgra, &mut rgba);
                        RgbaImage::from_raw(width, height, rgba).ok_or_else(|| failed("image"))
                    }
                    Ok(()) => Err(failed("no pixels")),
                    Err(error) => Err(failed(&error.to_string())),
                };
                SelectObject(memory, previous);
                let _ = DeleteObject(bitmap.into());
                image
            });
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        result
    }
}

fn bgra_to_opaque_rgba(bgra: &[u8], rgba: &mut [u8]) {
    for (o, i) in rgba.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
        o.copy_from_slice(&[i[2], i[1], i[0], 255]);
    }
}
