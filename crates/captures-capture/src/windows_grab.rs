//! Display stills on Windows, through GDI: about 65 ms at 4K whether or not
//! the desktop is changing, with no Direct3D device. xcap's
//! Windows.Graphics.Capture (WGC) grab left about 32 MB of GPU memory charged
//! to the process per grab at 4K on NVIDIA, never reclaimed, and any per-grab
//! WGC session does. DXGI desktop duplication gets no frame from a still
//! desktop, the usual case, so it waited 250 ms and fell back to GDI anyway.
//! GDI returns the same pixels as duplication.
//!
//! Windows whose display crop would show another window go through WGC, as
//! in xcap, but on a Direct3D device made for the grab and released after
//! it. xcap kept every grab on one device for the life of the process, which
//! held the last window-sized surface (33-42 MB at 4K) and the device.

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

/// The window xcap identifies as `id` (its `HWND`), through WGC: its
/// DWM-frame-bounds-sized top-left corner of the capture surface, RGBA with
/// alpha, without the cursor or the capture border. The same pixels as
/// xcap's `capture_image` for a per-monitor DPI-aware process like Captures.
pub(crate) fn capture_window(id: &str) -> CaptureResult<RgbaImage> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow, IsWindowVisible};

    let hwnd = id
        .parse::<u32>()
        .map(|handle| HWND(handle as usize as *mut _))
        .map_err(|_| CaptureError::TargetUnavailable)?;
    let mut bounds = RECT::default();
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool()
            || !IsWindowVisible(hwnd).as_bool()
            || IsIconic(hwnd).as_bool()
        {
            return Err(CaptureError::TargetUnavailable);
        }
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            std::ptr::from_mut(&mut bounds).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
        .map_err(|error| CaptureError::Backend(error.to_string()))?;
    }
    let width = u32::try_from(bounds.right - bounds.left).unwrap_or(0);
    let height = u32::try_from(bounds.bottom - bounds.top).unwrap_or(0);
    if width == 0 || height == 0 {
        return Err(CaptureError::TargetUnavailable);
    }
    wgc_window(hwnd, width, height).map_err(|error| CaptureError::Backend(error.to_string()))
}

fn wgc_window(
    hwnd: windows::Win32::Foundation::HWND,
    width: u32,
    height: u32,
) -> windows::core::Result<RgbaImage> {
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use windows::Foundation::TypedEventHandler;
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
    use windows::Graphics::DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat};
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
        D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
        D3D11CreateDevice, ID3D11Texture2D,
    };
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::System::WinRT::Direct3D11::{
        CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
    };
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::core::{Error, IInspectable, Interface, factory};

    let item: GraphicsCaptureItem = unsafe {
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?.CreateForWindow(hwnd)?
    };
    let (mut device, mut context) = (None, None);
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    let (Some(device), Some(context)) = (device, context) else {
        return Err(Error::empty());
    };
    let winrt: IDirect3DDevice =
        unsafe { CreateDirect3D11DeviceFromDXGIDevice(&device.cast::<IDXGIDevice>()?)?.cast()? };
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        1,
        item.Size()?,
    )?;
    let (sender, receiver) = channel();
    pool.FrameArrived(
        &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
            if let Some(frame) = pool.as_ref().and_then(|pool| pool.TryGetNextFrame().ok()) {
                let _ = sender.send(frame);
            }
            Ok(())
        }),
    )?;
    let session = pool.CreateCaptureSession(&item)?;
    let _ = session.SetIsBorderRequired(false);
    let _ = session.SetIsCursorCaptureEnabled(false);
    let image = session.StartCapture().and_then(|()| {
        let frame = receiver
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| Error::empty())?;
        let read = || -> windows::core::Result<RgbaImage> {
            let source: ID3D11Texture2D = unsafe {
                frame
                    .Surface()?
                    .cast::<IDirect3DDxgiInterfaceAccess>()?
                    .GetInterface()?
            };
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe { source.GetDesc(&mut desc) };
            if width > desc.Width || height > desc.Height {
                return Err(Error::empty());
            }
            let mut staging_desc = desc;
            staging_desc.Width = width;
            staging_desc.Height = height;
            staging_desc.BindFlags = 0;
            staging_desc.MiscFlags = 0;
            staging_desc.Usage = D3D11_USAGE_STAGING;
            staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            unsafe { device.CreateTexture2D(&staging_desc, None, Some(&mut staging))? };
            let staging = staging.ok_or_else(Error::empty)?;
            let row_bytes = width as usize * 4;
            let mut rgba = vec![0u8; row_bytes * height as usize];
            unsafe {
                context.CopySubresourceRegion(
                    &staging,
                    0,
                    0,
                    0,
                    0,
                    &source,
                    0,
                    Some(&D3D11_BOX {
                        left: 0,
                        top: 0,
                        front: 0,
                        right: width,
                        bottom: height,
                        back: 1,
                    }),
                );
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                let base = mapped.pData.cast::<u8>().cast_const();
                for (y, out) in rgba.chunks_exact_mut(row_bytes).enumerate() {
                    let row = std::slice::from_raw_parts(
                        base.add(y * mapped.RowPitch as usize),
                        row_bytes,
                    );
                    for (o, i) in out.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
                        o.copy_from_slice(&[i[2], i[1], i[0], i[3]]);
                    }
                }
                context.Unmap(&staging, 0);
            }
            RgbaImage::from_raw(width, height, rgba).ok_or_else(Error::empty)
        };
        let image = read();
        let _ = frame.Close();
        image
    });
    // Release everything the grab made, so the driver frees its surfaces.
    let _ = session.Close();
    let _ = pool.Close();
    let _ = winrt.Close();
    unsafe {
        context.ClearState();
        context.Flush();
    }
    image
}
