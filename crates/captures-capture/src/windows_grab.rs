//! Display stills on Windows without xcap's Windows.Graphics.Capture path,
//! which leaves one display-sized GPU surface behind per grab (32 MB at 4K on
//! NVIDIA, never reclaimed). DXGI desktop duplication on one kept D3D11 device
//! reads the same composed desktop; GDI, whose pixels match duplication's,
//! serves rotated outputs and anything duplication cannot.
use std::sync::Mutex;
use std::time::{Duration, Instant};

use image::RgbaImage;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1,
    IDXGIFactory1, IDXGIOutput1,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDC, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::core::Interface;

use crate::{CaptureError, CaptureResult};

/// Created on the first grab and kept: a device per grab costs ~150 ms.
struct Device {
    adapter: (u32, i32),
    device: ID3D11Device,
    context: ID3D11DeviceContext,
}

static DEVICE: Mutex<Option<Device>> = Mutex::new(None);

/// How long a new duplication may take to deliver its first composed frame
/// (about 20 ms here) before GDI reads the desktop instead.
const FIRST_FRAME: Duration = Duration::from_millis(250);

/// The desktop at physical rect `(x, y, width, height)`, without the cursor.
pub(crate) fn capture_display(x: i32, y: i32, width: u32, height: u32) -> CaptureResult<RgbaImage> {
    match duplicate(x, y, width, height) {
        Ok(Some(image)) => Ok(image),
        Ok(None) => gdi(x, y, width, height),
        Err(_) => {
            // A removed or reset device fails here; the next grab makes a new one.
            if let Ok(mut device) = DEVICE.lock() {
                *device = None;
            }
            gdi(x, y, width, height)
        }
    }
}

/// `None` when duplication cannot serve this output: no matching or a rotated
/// output, a different size, or no composed frame in time.
fn duplicate(x: i32, y: i32, width: u32, height: u32) -> windows::core::Result<Option<RgbaImage>> {
    let Some((adapter, output)) = find_output(x, y, width, height)? else {
        return Ok(None);
    };
    let luid = unsafe { adapter.GetDesc1()? }.AdapterLuid;
    let luid = (luid.LowPart, luid.HighPart);
    let mut slot = DEVICE.lock().map_err(|_| windows::core::Error::empty())?;
    if slot.as_ref().is_none_or(|device| device.adapter != luid) {
        let (mut device, mut context) = (None, None);
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
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
            return Ok(None);
        };
        *slot = Some(Device {
            adapter: luid,
            device,
            context,
        });
    }
    let Some(Device {
        device, context, ..
    }) = slot.as_ref()
    else {
        return Ok(None);
    };
    unsafe {
        let duplication = output.DuplicateOutput(device)?;
        // A new duplication's first frame can be empty (no present yet).
        let deadline = Instant::now() + FIRST_FRAME;
        let texture = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;
            match duplication.AcquireNextFrame(left.as_millis() as u32, &mut info, &mut resource) {
                Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                result => result?,
            }
            if info.LastPresentTime != 0
                && let Some(resource) = resource
            {
                break resource.cast::<ID3D11Texture2D>()?;
            }
            drop(resource);
            duplication.ReleaseFrame()?;
            if left.is_zero() {
                return Ok(None);
            }
        };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        if desc.Width != width || desc.Height != height || desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM
        {
            duplication.ReleaseFrame()?;
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
        let mut staging = None;
        device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
        let staging = staging.ok_or_else(windows::core::Error::empty)?;
        context.CopyResource(&staging, &texture);
        drop(texture);
        duplication.ReleaseFrame()?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let image = rgba_from_bgra(
            mapped.pData.cast::<u8>().cast_const(),
            mapped.RowPitch as usize,
            width,
            height,
        );
        context.Unmap(&staging, 0);
        Ok(image)
    }
}

/// The output whose desktop rect is exactly `(x, y, width, height)`, unrotated.
fn find_output(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> windows::core::Result<Option<(IDXGIAdapter1, IDXGIOutput1)>> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                let desc = output.GetDesc()?;
                let r = desc.DesktopCoordinates;
                if (r.left, r.top) == (x, y)
                    && i64::from(r.right - r.left) == i64::from(width)
                    && i64::from(r.bottom - r.top) == i64::from(height)
                {
                    let upright = desc.Rotation == DXGI_MODE_ROTATION_IDENTITY
                        || desc.Rotation == DXGI_MODE_ROTATION_UNSPECIFIED;
                    if !upright {
                        return Ok(None);
                    }
                    return Ok(Some((adapter, output.cast()?)));
                }
                o += 1;
            }
            a += 1;
        }
        Ok(None)
    }
}

/// The desktop through GDI, for outputs duplication does not serve.
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
                    Ok(()) if !bits.is_null() => rgba_from_bgra(
                        bits.cast::<u8>().cast_const(),
                        width as usize * 4,
                        width,
                        height,
                    )
                    .ok_or_else(|| failed("image")),
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

/// Opaque RGBA from `height` rows of BGRA `pitch` bytes apart.
///
/// # Safety
/// `source` must be readable for `pitch * (height - 1) + width * 4` bytes.
unsafe fn rgba_from_bgra(
    source: *const u8,
    pitch: usize,
    width: u32,
    height: u32,
) -> Option<RgbaImage> {
    let row = width as usize * 4;
    let mut rgba = vec![0u8; row * height as usize];
    for (y, out) in rgba.chunks_exact_mut(row).enumerate() {
        let input = unsafe { std::slice::from_raw_parts(source.add(y * pitch), row) };
        for (o, i) in out.chunks_exact_mut(4).zip(input.chunks_exact(4)) {
            o.copy_from_slice(&[i[2], i[1], i[0], 255]);
        }
    }
    RgbaImage::from_raw(width, height, rgba)
}
