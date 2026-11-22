//! Display stills on Windows. xcap's Windows.Graphics.Capture (WGC) grab left
//! about 32 MB of GPU memory charged to the process per grab at 4K on NVIDIA,
//! never reclaimed, and any per-grab WGC session does (a fresh item, pool and
//! session, all closed, still leaks the same). DXGI desktop duplication gets
//! no frame from a still desktop, the usual case, so it waited and fell back.
//! GDI reads a still or busy desktop alike, in about 65 ms at 4K, with the
//! same pixels as duplication and no Direct3D device. While a WGC session
//! is live, though, Windows draws the pointer in software and GDI includes
//! it, so stills taken during this process's recordings (a [`CaptureSession`])
//! use WGC instead. Other apps' capture sessions are not visible here.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::time::Duration;

use image::RgbaImage;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, POINT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDC, MONITOR_DEFAULTTONULL,
    MonitorFromPoint, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{AgileReference, IInspectable, Interface, factory};

use crate::{CaptureError, CaptureResult};

/// Created on the first grab during a recording and kept, as xcap kept its own.
struct Device {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    /// One WinRT wrapper for the device's life: a wrapper per grab, as xcap
    /// makes, kept that grab's frame surface alive. Agile, so any worker
    /// thread can use it.
    winrt: AgileReference<IDirect3DDevice>,
}

static DEVICE: Mutex<Option<Device>> = Mutex::new(None);

/// A new session's first frame arrives in about 10 ms, still desktop or not.
const FIRST_FRAME: Duration = Duration::from_secs(1);

static SESSIONS: AtomicUsize = AtomicUsize::new(0);

/// Held while this process runs a WGC capture session (a recording).
pub struct CaptureSession(());

impl CaptureSession {
    pub fn begin() -> Self {
        SESSIONS.fetch_add(1, Ordering::AcqRel);
        Self(())
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        SESSIONS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The desktop at physical rect `(x, y, width, height)`, without the cursor.
pub(crate) fn capture_display(x: i32, y: i32, width: u32, height: u32) -> CaptureResult<RgbaImage> {
    if SESSIONS.load(Ordering::Acquire) > 0 {
        match wgc(x, y, width, height) {
            Ok(Some(image)) => return Ok(image),
            Ok(None) => {}
            Err(_) => {
                // A removed or reset device fails here; the next grab makes a new one.
                if let Ok(mut device) = DEVICE.lock() {
                    *device = None;
                }
            }
        }
    }
    gdi(x, y, width, height)
}

/// `None` when no monitor or a different size covers the rect, or no frame came.
fn wgc(x: i32, y: i32, width: u32, height: u32) -> windows::core::Result<Option<RgbaImage>> {
    let (w, h) = (i32::try_from(width), i32::try_from(height));
    let (Ok(w), Ok(h)) = (w, h) else {
        return Ok(None);
    };
    let center = POINT {
        x: x + w / 2,
        y: y + h / 2,
    };
    let monitor = unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONULL) };
    if monitor.is_invalid() {
        return Ok(None);
    }
    let mut slot = DEVICE.lock().map_err(|_| windows::core::Error::empty())?;
    if slot.is_none() {
        *slot = Some(create_device()?);
    }
    let Some(Device {
        device,
        context,
        winrt,
    }) = slot.as_ref()
    else {
        return Ok(None);
    };
    let winrt = winrt.resolve()?;
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor)? };
    let size = item.Size()?;
    if (size.Width, size.Height) != (w, h) {
        return Ok(None);
    }
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        1,
        size,
    )?;
    let (arrived, frame_ready) = channel();
    let token = pool.FrameArrived(
        &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |_, _| {
            let _ = arrived.send(());
            Ok(())
        }),
    )?;
    let session = pool.CreateCaptureSession(&item)?;
    // Windows 11 and the graphicsCaptureWithoutBorder capability; as xcap,
    // capture anyway where the border cannot be turned off.
    let _ = session.SetIsBorderRequired(false);
    let _ = session.SetIsCursorCaptureEnabled(false);
    let image = session.StartCapture().and_then(|()| {
        if frame_ready.recv_timeout(FIRST_FRAME).is_err() {
            return Ok(None);
        }
        let frame = pool.TryGetNextFrame()?;
        let copied = copy_frame(device, context, &frame, width, height);
        let _ = frame.Close();
        copied
    });
    let _ = pool.RemoveFrameArrived(token);
    let _ = session.Close();
    let _ = pool.Close();
    image
}

fn create_device() -> windows::core::Result<Device> {
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
        return Err(windows::core::Error::empty());
    };
    let winrt: IDirect3DDevice =
        unsafe { CreateDirect3D11DeviceFromDXGIDevice(&device.cast::<IDXGIDevice>()?)? }.cast()?;
    Ok(Device {
        device,
        context,
        winrt: AgileReference::new(&winrt)?,
    })
}

fn copy_frame(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
    width: u32,
    height: u32,
) -> windows::core::Result<Option<RgbaImage>> {
    let surface = frame.Surface()?;
    let texture: ID3D11Texture2D = unsafe {
        surface
            .cast::<IDirect3DDxgiInterfaceAccess>()?
            .GetInterface()?
    };
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };
    if (desc.Width, desc.Height) != (width, height) {
        return Ok(None);
    }
    let staging_desc = D3D11_TEXTURE2D_DESC {
        MipLevels: 1,
        ArraySize: 1,
        BindFlags: 0,
        MiscFlags: 0,
        Usage: D3D11_USAGE_STAGING,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        ..desc
    };
    unsafe {
        let mut staging = None;
        device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
        let staging = staging.ok_or_else(windows::core::Error::empty)?;
        context.CopyResource(&staging, &texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let row = width as usize * 4;
        let pitch = mapped.RowPitch as usize;
        let source = std::slice::from_raw_parts(
            mapped.pData.cast::<u8>().cast_const(),
            pitch * (height as usize - 1) + row,
        );
        let mut rgba = vec![0u8; row * height as usize];
        for (y, out) in rgba.chunks_exact_mut(row).enumerate() {
            bgra_to_opaque_rgba(&source[y * pitch..][..row], out);
        }
        context.Unmap(&staging, 0);
        Ok(RgbaImage::from_raw(width, height, rgba))
    }
}

/// The desktop through GDI: every still outside a recording, and the fallback.
fn gdi(x: i32, y: i32, width: u32, height: u32) -> CaptureResult<RgbaImage> {
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
