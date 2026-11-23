//! Display recording frames on Windows through Windows.Graphics.Capture.
//!
//! xcap's WGC recorder read back every desktop refresh (up to 240 Hz)
//! through a new staging texture, into two fresh 33 MB buffers at 4K, and
//! swizzled in a separate byte loop, although the encoder only takes the
//! newest frame per recording tick: its capture thread was busy about 86%
//! of the time. This reader skips frames that arrive sooner than three
//! quarters of the recording interval after the last one it read, reuses
//! one staging texture and the buffers the encoder hands back, and
//! swizzles BGRA to RGBA during the row copy. Frames match xcap's: the
//! pool-sized surface, RGBA, alpha kept.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use image::RgbaImage;
use parking_lot::Mutex;
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat};
use windows::Win32::Foundation::{HMODULE, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11Multithread, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{BOOL, IInspectable, Interface, factory};
use xcap::Frame;

use crate::segment::{XcapRecordingError, XcapRecordingResult};

/// Frame buffers the encoder is done with, while a recording runs (`None`
/// otherwise). Reusing them saves faulting in 33 MB of fresh pages per 4K
/// frame, about 5 ms.
static SPARE: Mutex<Option<Vec<Vec<u8>>>> = Mutex::new(None);
const MAX_SPARE: usize = 3;

/// Takes back a frame's pixels for the next frame while recording.
pub(crate) fn recycle(buffer: Vec<u8>) {
    if let Some(spare) = SPARE.lock().as_mut()
        && spare.len() < MAX_SPARE
    {
        spare.push(buffer);
    }
}

fn frame_buffer(len: usize) -> Vec<u8> {
    let reused = SPARE.lock().as_mut().and_then(Vec::pop);
    reused
        .filter(|buffer| buffer.len() == len)
        .unwrap_or_else(|| vec![0; len])
}

pub(crate) struct WgcRecorder {
    session: GraphicsCaptureSession,
    frame_pool: Direct3D11CaptureFramePool,
    device: IDirect3DDevice,
    d3d: ID3D11Device,
    context: ID3D11DeviceContext,
    gpu_target: Arc<Mutex<Option<GpuTexture>>>,
    size: (u32, u32),
}

/// A texture shared between the frame pool's callback and the encoder,
/// always used with the device's multithread protection on.
#[derive(Clone)]
pub(crate) struct GpuTexture(pub(crate) ID3D11Texture2D);

// D3D11 resources are free-threaded; the device context is protected by
// `SetMultithreadProtected`.
unsafe impl Send for GpuTexture {}
unsafe impl Sync for GpuTexture {}

impl WgcRecorder {
    /// Starts capturing the display xcap identifies as `display_id`. Frames
    /// arrive on the receiver, which hands each one over before the next.
    pub(crate) fn start(
        display_id: &str,
        frames_per_second: u16,
    ) -> XcapRecordingResult<(Self, Receiver<Frame>)> {
        let monitor = find_monitor(display_id).ok_or(XcapRecordingError::TargetUnavailable)?;
        *SPARE.lock() = Some(Vec::new());
        Self::create(monitor, frames_per_second).map_err(|error| {
            *SPARE.lock() = None;
            XcapRecordingError::Capture(error.to_string())
        })
    }

    fn create(
        monitor: HMONITOR,
        frames_per_second: u16,
    ) -> windows::core::Result<(Self, Receiver<Frame>)> {
        let item: GraphicsCaptureItem = unsafe {
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?
                .CreateForMonitor(monitor)?
        };
        let (d3d, context) = create_device()?;
        let device = unsafe {
            CreateDirect3D11DeviceFromDXGIDevice(&d3d.cast::<IDXGIDevice>()?)?
                .cast::<IDirect3DDevice>()?
        };
        let size = item.Size()?;
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        let gpu_target = Arc::new(Mutex::new(None));
        // Three quarters of the recording interval: the encoder still finds
        // a new frame at every tick, and a 240 Hz desktop is read at about
        // 80 Hz for a 60 fps recording instead of 240 Hz.
        let min_gap = Duration::from_secs_f64(0.75 / f64::from(frames_per_second.max(1)));
        let (sender, receiver) = sync_channel(0);
        let reader = Reader {
            device: d3d.clone(),
            context: context.clone(),
            gpu_target: gpu_target.clone(),
            state: Mutex::new(ReaderState {
                staging: None,
                last_read: None,
            }),
            min_gap,
            sender,
        };
        frame_pool.FrameArrived(
            &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
                if let Some(pool) = pool.as_ref() {
                    reader.arrived(pool);
                }
                Ok(())
            }),
        )?;
        let session = frame_pool.CreateCaptureSession(&item)?;
        // Best effort, like xcap: older builds lack these.
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(false);
        // Windows 11 24H2 and later also stop composing the frames we skip.
        let _ = session.SetMinUpdateInterval(TimeSpan {
            Duration: i64::try_from(min_gap.as_nanos() / 100).unwrap_or(0),
        });
        let recorder = Self {
            session,
            frame_pool,
            device,
            d3d,
            context,
            gpu_target,
            size: (
                u32::try_from(size.Width).unwrap_or(0),
                u32::try_from(size.Height).unwrap_or(0),
            ),
        };
        recorder.session.StartCapture()?;
        Ok((recorder, receiver))
    }

    pub(crate) fn device(&self) -> (&ID3D11Device, &ID3D11DeviceContext) {
        (&self.d3d, &self.context)
    }

    /// Stops reading frames back to the CPU. From now on each frame is
    /// copied on the GPU into the returned texture, which starts out as
    /// `first` (the frame already received) and stays pool-sized.
    pub(crate) fn keep_frames_on_gpu(
        &self,
        first: &RgbaImage,
    ) -> windows::core::Result<GpuTexture> {
        let (width, height) = self.size;
        if (first.width(), first.height()) != (width, height) {
            return Err(windows::core::Error::empty());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            ..Default::default()
        };
        let mut bgra = first.as_raw().clone();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: bgra.as_ptr().cast(),
            SysMemPitch: width * 4,
            SysMemSlicePitch: 0,
        };
        let mut texture = None;
        unsafe {
            self.d3d
                .CreateTexture2D(&desc, Some(&initial), Some(&mut texture))?;
        }
        let texture = GpuTexture(texture.ok_or_else(windows::core::Error::empty)?);
        *self.gpu_target.lock() = Some(texture.clone());
        Ok(texture)
    }

    pub(crate) fn stop(&self) -> windows::core::Result<()> {
        *SPARE.lock() = None;
        self.session.Close()?;
        self.frame_pool.Close()?;
        self.device.Close()
    }
}

impl Drop for WgcRecorder {
    fn drop(&mut self) {
        // `Close` is idempotent, so a stopped recorder closes again harmlessly.
        let _ = self.stop();
    }
}

/// The frame pool's callback state.
struct Reader {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    /// Set once the hardware encoder takes frames on the GPU.
    gpu_target: Arc<Mutex<Option<GpuTexture>>>,
    state: Mutex<ReaderState>,
    min_gap: Duration,
    sender: SyncSender<Frame>,
}

struct ReaderState {
    /// Reused while the surface size stays the same.
    staging: Option<Staging>,
    last_read: Option<Instant>,
}

struct Staging {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
}

// The texture is only used under the mutex, from the frame pool's callback.
unsafe impl Send for Staging {}

impl Reader {
    fn arrived(&self, pool: &Direct3D11CaptureFramePool) {
        let Ok(frame) = pool.TryGetNextFrame() else {
            return;
        };
        let image = {
            let mut state = self.state.lock();
            let now = Instant::now();
            if state
                .last_read
                .is_some_and(|last| now.duration_since(last) < self.min_gap)
            {
                None
            } else {
                state.last_read = Some(now);
                let target = self.gpu_target.lock().clone();
                match target {
                    Some(target) => {
                        let _ = self.copy(&target, &frame);
                        state.staging = None;
                        None
                    }
                    None => self.read(&mut state.staging, &frame).ok(),
                }
            }
        };
        // Return the surface to the pool before waiting on the receiver.
        let _ = frame.Close();
        if let Some(image) = image {
            let _ = self.sender.send(image);
        }
    }

    fn copy(
        &self,
        target: &GpuTexture,
        frame: &Direct3D11CaptureFrame,
    ) -> windows::core::Result<()> {
        let source: ID3D11Texture2D = unsafe {
            frame
                .Surface()?
                .cast::<IDirect3DDxgiInterfaceAccess>()?
                .GetInterface()?
        };
        unsafe { self.context.CopyResource(&target.0, &source) };
        Ok(())
    }

    fn read(
        &self,
        staging: &mut Option<Staging>,
        frame: &Direct3D11CaptureFrame,
    ) -> windows::core::Result<Frame> {
        let source: ID3D11Texture2D = unsafe {
            frame
                .Surface()?
                .cast::<IDirect3DDxgiInterfaceAccess>()?
                .GetInterface()?
        };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { source.GetDesc(&mut desc) };
        if !staging
            .as_ref()
            .is_some_and(|s| s.width == desc.Width && s.height == desc.Height)
        {
            let mut staging_desc = desc;
            staging_desc.BindFlags = 0;
            staging_desc.MiscFlags = 0;
            staging_desc.Usage = D3D11_USAGE_STAGING;
            staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut texture = None;
            unsafe {
                self.device
                    .CreateTexture2D(&staging_desc, None, Some(&mut texture))?;
            }
            *staging = texture.map(|texture| Staging {
                texture,
                width: desc.Width,
                height: desc.Height,
            });
        }
        let texture = &staging
            .as_ref()
            .ok_or_else(windows::core::Error::empty)?
            .texture;
        let row_bytes = desc.Width as usize * 4;
        let mut rgba = frame_buffer(row_bytes * desc.Height as usize);
        unsafe {
            self.context.CopyResource(texture, &source);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let base = mapped.pData.cast::<u8>().cast_const();
            for (y, out) in rgba.chunks_exact_mut(row_bytes).enumerate() {
                let row =
                    std::slice::from_raw_parts(base.add(y * mapped.RowPitch as usize), row_bytes);
                bgra_to_rgba(row, out);
            }
            self.context.Unmap(texture, 0);
        }
        Ok(Frame::new(desc.Width, desc.Height, rgba))
    }
}

/// Swaps red and blue, keeping alpha. Four pixels per step as `u32`s, which
/// the compiler vectorizes: 3.6 ms at 4K against 6.9 ms byte by byte.
fn bgra_to_rgba(bgra: &[u8], rgba: &mut [u8]) {
    fn swap(i: &[u8], o: &mut [u8]) {
        let p = u32::from_le_bytes([i[0], i[1], i[2], i[3]]);
        let q = (p & 0xFF00_FF00) | ((p >> 16) & 0xFF) | ((p & 0xFF) << 16);
        o.copy_from_slice(&q.to_le_bytes());
    }
    let mut out = rgba.chunks_exact_mut(16);
    let mut pixels = bgra.chunks_exact(16);
    for (o, i) in (&mut out).zip(&mut pixels) {
        for k in 0..4 {
            swap(&i[k * 4..k * 4 + 4], &mut o[k * 4..k * 4 + 4]);
        }
    }
    let rest = pixels.remainder().chunks_exact(4);
    for (o, i) in out.into_remainder().chunks_exact_mut(4).zip(rest) {
        swap(i, o);
    }
}

fn create_device() -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    // Video support lets the hardware encoder convert and encode frames on
    // this device; drivers without it still capture.
    for flags in [
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    ] {
        let created = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                flags,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        if created.is_ok() || flags == D3D11_CREATE_DEVICE_BGRA_SUPPORT {
            created?;
            break;
        }
    }
    let (Some(device), Some(context)) = (device, context) else {
        return Err(windows::core::Error::empty());
    };
    // The frame pool uses the device from its own threads too.
    if let Ok(multithread) = device.cast::<ID3D11Multithread>() {
        let _ = unsafe { multithread.SetMultithreadProtected(true) };
    }
    Ok((device, context))
}

/// The monitor whose handle xcap reports as its id (`HMONITOR as u32`).
fn find_monitor(display_id: &str) -> Option<HMONITOR> {
    unsafe extern "system" fn each(monitor: HMONITOR, _: HDC, _: *mut RECT, found: LPARAM) -> BOOL {
        let found = unsafe { &mut *(found.0 as *mut (String, Option<HMONITOR>)) };
        if (monitor.0 as u32).to_string() == found.0 {
            found.1 = Some(monitor);
            return false.into();
        }
        true.into()
    }
    let mut found = (display_id.to_owned(), None);
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(each),
            LPARAM(std::ptr::from_mut(&mut found) as isize),
        );
    }
    found.1
}

#[cfg(test)]
mod tests {
    #[test]
    fn swizzle_matches_a_byte_swap_for_every_length() {
        // 17 pixels: four vectorized steps plus a one-pixel remainder.
        let bgra: Vec<u8> = (0..68u8).map(|v| v.wrapping_mul(37)).collect();
        let mut out = vec![0; bgra.len()];
        super::bgra_to_rgba(&bgra, &mut out);
        let expected: Vec<u8> = bgra
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        assert_eq!(out, expected);
    }
}
