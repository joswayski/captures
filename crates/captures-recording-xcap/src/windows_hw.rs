//! Hardware H.264 display recording on Windows.
//!
//! The software path reads every frame back from the GPU, swizzles it,
//! scales it and converts it to YUV on the CPU, then encodes it with
//! openh264: about 236% of a core at 4K/60, which still encoded only about
//! 16 frames a second. Here frames never leave the GPU, as in ShadowPlay or
//! OBS: the capture surface is copied into a texture the encoder owns, the
//! D3D11 video processor crops, scales and converts it to NV12 (BT.601,
//! limited range, as the software path labels), and the Media Foundation
//! hardware H.264 encoder (NVENC, AMF or Quick Sync, found through
//! `MFTEnum2` without vendor SDKs) encodes it from the shared device. The
//! bitstream is muxed with the same `mp4` crate settings as captures-video.
//!
//! Click highlights and the cursor are drawn by the same `PointerOverlay`
//! code as the software path, on the CPU, over only the pixels it touches:
//! that small rectangle is read back, drawn on and uploaded before the
//! conversion to NV12.
//!
//! When there is no hardware encoder (virtual machines, Remote Desktop,
//! old GPUs) or anything here fails to start, recording falls back to the
//! software path.

use std::fs::{self, File};
use std::io::Write;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use image::RgbaImage;
use mp4::{
    AvcConfig, FourCC, MediaConfig, Mp4Config, Mp4Sample, Mp4Writer, TrackConfig, TrackType,
};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_OPTIMAL_QUALITY, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize, CODECAPI_AVLowLatencyMode,
    ICodecAPI, IMF2DBuffer, IMFActivate, IMFAttributes, IMFDXGIDeviceManager,
    IMFMediaEventGenerator, IMFMediaType, IMFSample, IMFTransform, METransformDrainComplete,
    METransformHaveOutput, METransformNeedInput, MF_E_NO_EVENTS_AVAILABLE,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_EVENT_FLAG_NO_WAIT, MF_EVENT_TYPE, MF_MT_AVG_BITRATE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_MT_TRANSFER_FUNCTION, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_PRIMARIES, MF_MT_YUV_MATRIX,
    MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION, MFCreateAttributes,
    MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer, MFCreateMediaType, MFCreateSample,
    MFMediaType_Video, MFNominalRange_16_235, MFSTARTUP_NOSOCKET, MFSampleExtension_CleanPoint,
    MFShutdown, MFStartup, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_ADAPTER_LUID,
    MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER, MFT_MESSAGE_COMMAND_DRAIN,
    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_END_OF_STREAM,
    MFT_MESSAGE_NOTIFY_END_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MFT_REGISTER_TYPE_INFO, MFTEnum2, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive, MFVideoPrimaries_SMPTE170M, MFVideoTransFunc_709,
    MFVideoTransferMatrix_BT601, eAVEncCommonRateControlMode_UnconstrainedVBR,
    eAVEncH264VProfile_Main,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Variant::VARIANT;
use windows::core::{Error, Interface, Result};

use crate::overlay::PointerOverlay;
use crate::pointer::PointerSource;
use crate::segment::{WorkerOutcome, XcapRecordingError, XcapRecordingResult};
use crate::transform::FrameRect;
use crate::windows_wgc::{GpuTexture, WgcRecorder};

const MP4_TIMESCALE: u32 = 90_000;
/// NV12 surfaces in flight. The encoder holds an input until its output
/// comes back, normally the next event after it.
const RING: usize = 4;
/// How long a frame waits for the encoder to accept input before it is
/// counted as skipped, like openh264's skipped frames.
const INPUT_WAIT: Duration = Duration::from_millis(20);
const DRAIN_WAIT: Duration = Duration::from_secs(3);

/// BT.601 limited range, the colour the software path converts to and labels.
const YUV_601_LIMITED: D3D11_VIDEO_PROCESSOR_COLOR_SPACE = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
    // Nominal_Range (bits 4-5) = 16-235; YCbCr_Matrix (bit 2) = 0, BT.601.
    _bitfield: 1 << 4,
};
/// Full-range RGB, as WGC delivers it.
const RGB_FULL: D3D11_VIDEO_PROCESSOR_COLOR_SPACE =
    D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };

/// The frame and pointer work of one recording; runs on its encoder thread.
pub(crate) struct HardwareEncoder {
    gpu: Gpu,
    mft: Mft,
    mux: Mux,
    overlay: Option<OverlayCanvas>,
    frame_rate: u16,
    inputs: u64,
    outputs: u64,
    skipped: u64,
}

// The COM objects are free-threaded (the device has multithread protection
// on and the encoder is an asynchronous MFT); they move to the encoder
// thread once and are used only there.
unsafe impl Send for HardwareEncoder {}

impl HardwareEncoder {
    /// Sets up hardware encoding of `source` (in capture-surface pixels)
    /// scaled to `output`, or `None` when this machine cannot. On success the
    /// recorder keeps its frames on the GPU from now on.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create(
        recorder: &WgcRecorder,
        first: &RgbaImage,
        source: FrameRect,
        output: (u32, u32),
        frame_rate: u16,
        bitrate: u32,
        output_path: &Path,
        overlay: bool,
    ) -> Option<Self> {
        let (device, context) = recorder.device();
        let mft = Mft::create(device, output, frame_rate, bitrate).ok()?;
        let frames = (first.width(), first.height());
        // Probe the conversion before switching the recorder over.
        let gpu = Gpu::create(device, context, frames, source, output, frame_rate, overlay).ok()?;
        let latest = recorder.keep_frames_on_gpu(first).ok()?;
        let mut encoder = Self {
            gpu,
            mft,
            mux: Mux::new(output_path, output, frame_rate),
            overlay: overlay.then(|| OverlayCanvas::new(output)),
            frame_rate,
            inputs: 0,
            outputs: 0,
            skipped: 0,
        };
        encoder.gpu.set_source(&latest).ok()?;
        Some(encoder)
    }

    /// Encodes the newest captured frame at `timestamp_ms`.
    fn encode(
        &mut self,
        timestamp_ms: u64,
        pointer: Option<(&mut PointerOverlay, &PointerSource)>,
    ) -> Result<()> {
        self.pump()?;
        let deadline = Instant::now() + INPUT_WAIT;
        while self.mft.need_input == 0 || self.in_flight() >= RING as u64 - 1 {
            if Instant::now() >= deadline {
                self.skipped += 1;
                return Ok(());
            }
            thread::sleep(Duration::from_millis(1));
            self.pump()?;
        }
        let slot = (self.inputs % RING as u64) as usize;
        let texture = {
            let _lock = self.gpu.lock();
            match (pointer, self.overlay.as_mut()) {
                (Some((overlay, source)), Some(canvas)) => {
                    self.gpu
                        .convert_with_overlay(slot, canvas, overlay, source)?;
                }
                _ => self.gpu.convert(slot)?,
            }
            self.gpu.ring[slot].0.clone()
        };
        let sample = unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &texture, 0, false)?;
            if let Ok(two_d) = buffer.cast::<IMF2DBuffer>() {
                buffer.SetCurrentLength(two_d.GetContiguousLength()?)?;
            }
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(i64::try_from(timestamp_ms).unwrap_or(i64::MAX) * 10_000)?;
            sample.SetSampleDuration(10_000_000 / i64::from(self.frame_rate.max(1)))?;
            sample
        };
        unsafe {
            self.mft
                .transform
                .ProcessInput(self.mft.input_id, &sample, 0)?;
        }
        self.mft.need_input -= 1;
        self.inputs += 1;
        self.pump().map(drop)
    }

    fn in_flight(&self) -> u64 {
        self.inputs.saturating_sub(self.outputs)
    }

    /// Handles the encoder's queued events without waiting.
    fn pump(&mut self) -> Result<bool> {
        let mut drained = false;
        loop {
            let event = match unsafe { self.mft.events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => event,
                Err(error) if error.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(drained),
                Err(error) => return Err(error),
            };
            let kind = MF_EVENT_TYPE(unsafe { event.GetType()? } as i32);
            unsafe { event.GetStatus()?.ok()? };
            if kind == METransformNeedInput {
                self.mft.need_input += 1;
            } else if kind == METransformHaveOutput {
                if let Some(output) = self.mft.output()? {
                    self.outputs += 1;
                    self.mux.write(&output, &self.mft)?;
                }
            } else if kind == METransformDrainComplete {
                drained = true;
            }
        }
    }

    fn finish(mut self, end_ms: u64) -> Result<(u64, u64)> {
        unsafe {
            let _ = self
                .mft
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            self.mft
                .transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)?;
        }
        let deadline = Instant::now() + DRAIN_WAIT;
        while !self.pump()? && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        self.mux.finish(end_ms)?;
        Ok((self.mux.encoded, self.skipped))
    }
}

/// Mirrors `record_frames`: one frame per recording tick from the newest
/// capture, with late ticks and encoder skips counted as dropped frames.
pub(crate) fn record_frames(
    mut encoder: HardwareEncoder,
    frame_rate: u16,
    started_at: Instant,
    mut pointer_overlay: Option<PointerOverlay>,
    stop_requested: &AtomicBool,
) -> XcapRecordingResult<WorkerOutcome> {
    let failed = |error: Error| XcapRecordingError::Worker(error.to_string());
    let frame_interval = Duration::from_secs_f64(1.0 / f64::from(frame_rate));
    let pointer_source = pointer_overlay.as_ref().map(|_| PointerSource::new());
    let mut skipped_ticks = 0_u64;
    encoder
        .encode(0, pointer_overlay.as_mut().zip(pointer_source.as_ref()))
        .map_err(failed)?;
    let mut next_frame_at = started_at + frame_interval;
    while !stop_requested.load(Ordering::Acquire) {
        let now = Instant::now();
        if now < next_frame_at {
            thread::sleep((next_frame_at - now).min(Duration::from_millis(10)));
            continue;
        }
        let elapsed_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        encoder
            .encode(
                elapsed_ms,
                pointer_overlay.as_mut().zip(pointer_source.as_ref()),
            )
            .map_err(failed)?;
        next_frame_at += frame_interval;
        let now = Instant::now();
        if now > next_frame_at {
            let missed = ((now - next_frame_at).as_secs_f64() / frame_interval.as_secs_f64())
                .floor() as u64
                + 1;
            skipped_ticks = skipped_ticks.saturating_add(missed);
            next_frame_at += frame_interval.mul_f64(missed as f64);
        }
    }
    let duration_ms = u64::try_from(started_at.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .max(1);
    let (encoded, skipped) = encoder.finish(duration_ms).map_err(failed)?;
    if encoded == 0 {
        return Err(XcapRecordingError::Worker(
            "the hardware encoder produced no frames".to_owned(),
        ));
    }
    Ok(WorkerOutcome {
        duration_ms,
        dropped_frames: skipped_ticks.saturating_add(skipped),
    })
}

/// The video processors and textures that turn capture frames into NV12.
struct Gpu {
    context: ID3D11DeviceContext,
    multithread: ID3D11Multithread,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    /// Capture frame → NV12, or → the BGRA canvas when drawing a pointer.
    scale: (ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor),
    /// BGRA canvas → NV12, only when drawing a pointer.
    convert: Option<(ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor)>,
    source_view: Option<ID3D11VideoProcessorInputView>,
    ring: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    canvas: Option<Canvas>,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    output: (u32, u32),
}

struct Canvas {
    texture: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
    input_view: ID3D11VideoProcessorInputView,
}

struct MultithreadLock(ID3D11Multithread);

impl Drop for MultithreadLock {
    fn drop(&mut self) {
        unsafe { self.0.Leave() };
    }
}

impl Gpu {
    fn create(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        frames: (u32, u32),
        source: FrameRect,
        output: (u32, u32),
        frame_rate: u16,
        overlay: bool,
    ) -> Result<Self> {
        let video_device: ID3D11VideoDevice = device.cast()?;
        let video_context: ID3D11VideoContext = context.cast()?;
        let multithread: ID3D11Multithread = context.cast()?;
        let scale = processor(&video_device, frames, output, frame_rate)?;
        let scale_target = if overlay {
            DXGI_FORMAT_B8G8R8A8_UNORM
        } else {
            DXGI_FORMAT_NV12
        };
        check_formats(&scale.0, scale_target)?;
        let source_rect = RECT {
            left: source.x as i32,
            top: source.y as i32,
            right: (source.x + source.width) as i32,
            bottom: (source.y + source.height) as i32,
        };
        let output_rect = RECT {
            left: 0,
            top: 0,
            right: output.0 as i32,
            bottom: output.1 as i32,
        };
        let out_space = if overlay { RGB_FULL } else { YUV_601_LIMITED };
        configure(
            &video_context,
            &scale.1,
            &source_rect,
            &output_rect,
            &out_space,
        );
        let convert = if overlay {
            let convert = processor(&video_device, output, output, frame_rate)?;
            check_formats(&convert.0, DXGI_FORMAT_NV12)?;
            configure(
                &video_context,
                &convert.1,
                &output_rect,
                &output_rect,
                &YUV_601_LIMITED,
            );
            Some(convert)
        } else {
            None
        };
        // Each NV12 view belongs to the processor that writes it.
        let nv12_writer = convert
            .as_ref()
            .map_or(&scale.0, |(enumerator, _)| enumerator);
        let mut ring = Vec::with_capacity(RING);
        for _ in 0..RING {
            let texture = texture(device, output, DXGI_FORMAT_NV12)?;
            let view = output_view(&video_device, &texture, nv12_writer)?;
            ring.push((texture, view));
        }
        let canvas = if overlay {
            let texture = texture(device, output, DXGI_FORMAT_B8G8R8A8_UNORM)?;
            let output_view = output_view(&video_device, &texture, &scale.0)?;
            let input_view = input_view(
                &video_device,
                &texture,
                &convert.as_ref().ok_or_else(Error::empty)?.0,
            )?;
            Some(Canvas {
                texture,
                output_view,
                input_view,
            })
        } else {
            None
        };
        Ok(Self {
            context: context.clone(),
            multithread,
            video_device,
            video_context,
            scale,
            convert,
            source_view: None,
            ring,
            canvas,
            staging: None,
            output,
        })
    }

    fn set_source(&mut self, latest: &GpuTexture) -> Result<()> {
        self.source_view = Some(input_view(&self.video_device, &latest.0, &self.scale.0)?);
        Ok(())
    }

    fn lock(&self) -> MultithreadLock {
        unsafe { self.multithread.Enter() };
        MultithreadLock(self.multithread.clone())
    }

    fn convert(&self, slot: usize) -> Result<()> {
        let source = self.source_view.as_ref().ok_or_else(Error::empty)?;
        blt(
            &self.video_context,
            &self.scale.1,
            source,
            &self.ring[slot].1,
        )
    }

    fn convert_with_overlay(
        &mut self,
        slot: usize,
        canvas: &mut OverlayCanvas,
        overlay: &mut PointerOverlay,
        pointer: &PointerSource,
    ) -> Result<()> {
        let source = self.source_view.as_ref().ok_or_else(Error::empty)?;
        let target = self.canvas.as_ref().ok_or_else(Error::empty)?;
        blt(
            &self.video_context,
            &self.scale.1,
            source,
            &target.output_view,
        )?;
        let sample = pointer.sample();
        let cursor = overlay
            .shows_cursor()
            .then(|| pointer.cursor_image())
            .flatten();
        let now = Instant::now();
        // The first draw finds the pixels the overlay covers this frame; the
        // second, identical for the same sample and time, draws over the
        // real frame read back from the GPU.
        let footprint = overlay.draw(&mut canvas.rgb, sample, cursor.as_ref(), now);
        if let Some(bounds) = footprint.bounds(self.output.0) {
            let texture = target.texture.clone();
            self.read_region(&texture, bounds, &mut canvas.rgb)?;
            let _ = overlay.draw(&mut canvas.rgb, sample, cursor.as_ref(), now);
            self.write_region(&texture, bounds, &canvas.rgb, &mut canvas.upload);
        }
        let (_, convert) = self.convert.as_ref().ok_or_else(Error::empty)?;
        let target = self.canvas.as_ref().ok_or_else(Error::empty)?;
        blt(
            &self.video_context,
            convert,
            &target.input_view,
            &self.ring[slot].1,
        )
    }

    /// Copies `bounds` of the BGRA canvas texture into the RGB canvas.
    fn read_region(
        &mut self,
        texture: &ID3D11Texture2D,
        (left, top, right, bottom): (u32, u32, u32, u32),
        rgb: &mut [u8],
    ) -> Result<()> {
        let (width, height) = (right - left, bottom - top);
        if !self
            .staging
            .as_ref()
            .is_some_and(|(_, w, h)| *w >= width && *h >= height)
        {
            let device = unsafe { self.context.GetDevice()? };
            let size = (width.max(64), height.max(64));
            let desc = D3D11_TEXTURE2D_DESC {
                Width: size.0,
                Height: size.1,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..Default::default()
            };
            let mut staging = None;
            unsafe { device.CreateTexture2D(&desc, None, Some(&mut staging))? };
            self.staging = Some((staging.ok_or_else(Error::empty)?, size.0, size.1));
        }
        let (staging, _, _) = self.staging.as_ref().ok_or_else(Error::empty)?;
        let region = D3D11_BOX {
            left,
            top,
            front: 0,
            right,
            bottom,
            back: 1,
        };
        let stride = self.output.0 as usize * 3;
        unsafe {
            self.context
                .CopySubresourceRegion(staging, 0, 0, 0, 0, texture, 0, Some(&region));
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let base = mapped.pData.cast::<u8>().cast_const();
            for row in 0..height as usize {
                let bgra = std::slice::from_raw_parts(
                    base.add(row * mapped.RowPitch as usize),
                    width as usize * 4,
                );
                let start = (top as usize + row) * stride + left as usize * 3;
                let out = &mut rgb[start..start + width as usize * 3];
                for (pixel, out) in bgra.chunks_exact(4).zip(out.chunks_exact_mut(3)) {
                    out.copy_from_slice(&[pixel[2], pixel[1], pixel[0]]);
                }
            }
            self.context.Unmap(staging, 0);
        }
        Ok(())
    }

    /// Uploads `bounds` of the RGB canvas into the BGRA canvas texture.
    fn write_region(
        &self,
        texture: &ID3D11Texture2D,
        (left, top, right, bottom): (u32, u32, u32, u32),
        rgb: &[u8],
        upload: &mut Vec<u8>,
    ) {
        let (width, height) = ((right - left) as usize, (bottom - top) as usize);
        let stride = self.output.0 as usize * 3;
        upload.clear();
        for row in 0..height {
            let start = (top as usize + row) * stride + left as usize * 3;
            for pixel in rgb[start..start + width * 3].chunks_exact(3) {
                upload.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
        }
        let region = D3D11_BOX {
            left,
            top,
            front: 0,
            right,
            bottom,
            back: 1,
        };
        unsafe {
            self.context.UpdateSubresource(
                texture,
                0,
                Some(&region),
                upload.as_ptr().cast(),
                (width * 4) as u32,
                0,
            );
        }
    }
}

/// The CPU side of the pointer overlay: an output-sized RGB frame that only
/// ever holds the pixels around the pointer (the rest stays untouched,
/// uncommitted zero pages).
struct OverlayCanvas {
    rgb: Vec<u8>,
    upload: Vec<u8>,
}

impl OverlayCanvas {
    fn new((width, height): (u32, u32)) -> Self {
        Self {
            rgb: vec![0; width as usize * height as usize * 3],
            upload: Vec::new(),
        }
    }
}

fn processor(
    video_device: &ID3D11VideoDevice,
    input: (u32, u32),
    output: (u32, u32),
    frame_rate: u16,
) -> Result<(ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor)> {
    let rate = DXGI_RATIONAL {
        Numerator: u32::from(frame_rate),
        Denominator: 1,
    };
    let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
        InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
        InputFrameRate: rate,
        InputWidth: input.0,
        InputHeight: input.1,
        OutputFrameRate: rate,
        OutputWidth: output.0,
        OutputHeight: output.1,
        Usage: D3D11_VIDEO_USAGE_OPTIMAL_QUALITY,
    };
    unsafe {
        let enumerator = video_device.CreateVideoProcessorEnumerator(&desc)?;
        let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;
        Ok((enumerator, processor))
    }
}

fn check_formats(enumerator: &ID3D11VideoProcessorEnumerator, output: DXGI_FORMAT) -> Result<()> {
    // D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT = 1, _OUTPUT = 2.
    let input = unsafe { enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_B8G8R8A8_UNORM)? };
    let target = unsafe { enumerator.CheckVideoProcessorFormat(output)? };
    if input & 1 == 0 || target & 2 == 0 {
        return Err(Error::empty());
    }
    Ok(())
}

fn configure(
    video_context: &ID3D11VideoContext,
    processor: &ID3D11VideoProcessor,
    source: &RECT,
    output: &RECT,
    output_space: &D3D11_VIDEO_PROCESSOR_COLOR_SPACE,
) {
    unsafe {
        video_context.VideoProcessorSetStreamFrameFormat(
            processor,
            0,
            D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
        );
        video_context.VideoProcessorSetStreamAutoProcessingMode(processor, 0, false);
        video_context.VideoProcessorSetStreamSourceRect(processor, 0, true, Some(source));
        video_context.VideoProcessorSetStreamDestRect(processor, 0, true, Some(output));
        video_context.VideoProcessorSetOutputTargetRect(processor, true, Some(output));
        video_context.VideoProcessorSetStreamColorSpace(processor, 0, &RGB_FULL);
        video_context.VideoProcessorSetOutputColorSpace(processor, output_space);
    }
}

fn blt(
    video_context: &ID3D11VideoContext,
    processor: &ID3D11VideoProcessor,
    input: &ID3D11VideoProcessorInputView,
    output: &ID3D11VideoProcessorOutputView,
) -> Result<()> {
    let stream = D3D11_VIDEO_PROCESSOR_STREAM {
        Enable: true.into(),
        pInputSurface: ManuallyDrop::new(Some(input.clone())),
        ..Default::default()
    };
    let result = unsafe {
        video_context.VideoProcessorBlt(processor, output, 0, std::slice::from_ref(&stream))
    };
    drop(ManuallyDrop::into_inner(stream.pInputSurface));
    result
}

fn texture(
    device: &ID3D11Device,
    (width, height): (u32, u32),
    format: DXGI_FORMAT,
) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        ..Default::default()
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
    texture.ok_or_else(Error::empty)
}

fn input_view(
    video_device: &ID3D11VideoDevice,
    texture: &ID3D11Texture2D,
    enumerator: &ID3D11VideoProcessorEnumerator,
) -> Result<ID3D11VideoProcessorInputView> {
    let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
        FourCC: 0,
        ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_VPIV {
                MipSlice: 0,
                ArraySlice: 0,
            },
        },
    };
    let mut view = None;
    unsafe {
        video_device.CreateVideoProcessorInputView(texture, enumerator, &desc, Some(&mut view))?
    };
    view.ok_or_else(Error::empty)
}

fn output_view(
    video_device: &ID3D11VideoDevice,
    texture: &ID3D11Texture2D,
    enumerator: &ID3D11VideoProcessorEnumerator,
) -> Result<ID3D11VideoProcessorOutputView> {
    let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
        ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
        },
    };
    let mut view = None;
    unsafe {
        video_device.CreateVideoProcessorOutputView(texture, enumerator, &desc, Some(&mut view))?;
    }
    view.ok_or_else(Error::empty)
}

/// An asynchronous hardware H.264 encoder MFT on the capture device.
struct Mft {
    activate: IMFActivate,
    transform: IMFTransform,
    events: IMFMediaEventGenerator,
    _manager: IMFDXGIDeviceManager,
    input_id: u32,
    output_id: u32,
    provides_samples: bool,
    output_size: u32,
    need_input: u32,
}

impl Mft {
    fn create(
        device: &ID3D11Device,
        output: (u32, u32),
        frame_rate: u16,
        bitrate: u32,
    ) -> Result<Self> {
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)? };
        let created = Self::find(device, output, frame_rate, bitrate);
        if created.is_err() {
            unsafe {
                let _ = MFShutdown();
            }
        }
        created
    }

    fn find(
        device: &ID3D11Device,
        output: (u32, u32),
        frame_rate: u16,
        bitrate: u32,
    ) -> Result<Self> {
        let luid = unsafe {
            device
                .cast::<IDXGIDevice>()?
                .GetAdapter()?
                .GetDesc()?
                .AdapterLuid
        };
        let mut attributes: Option<IMFAttributes> = None;
        unsafe { MFCreateAttributes(&mut attributes, 1)? };
        let attributes = attributes.ok_or_else(Error::empty)?;
        let luid_bytes: [u8; 8] = unsafe { std::mem::transmute(luid) };
        unsafe { attributes.SetBlob(&MFT_ENUM_ADAPTER_LUID, &luid_bytes)? };
        let input_info = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_NV12,
        };
        let output_info = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };
        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0;
        unsafe {
            MFTEnum2(
                MFT_CATEGORY_VIDEO_ENCODER,
                MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
                Some(&input_info),
                Some(&output_info),
                &attributes,
                &mut list,
                &mut count,
            )?;
        }
        let activates: Vec<IMFActivate> = if list.is_null() {
            Vec::new()
        } else {
            let taken = unsafe { std::slice::from_raw_parts_mut(list, count as usize) }
                .iter_mut()
                .filter_map(Option::take)
                .collect();
            unsafe { CoTaskMemFree(Some(list.cast())) };
            taken
        };
        let mut last = Error::empty();
        for activate in activates {
            match Self::open(activate.clone(), device, output, frame_rate, bitrate) {
                Ok(mft) => return Ok(mft),
                Err(error) => {
                    unsafe {
                        let _ = activate.ShutdownObject();
                    }
                    last = error;
                }
            }
        }
        Err(last)
    }

    fn open(
        activate: IMFActivate,
        device: &ID3D11Device,
        (width, height): (u32, u32),
        frame_rate: u16,
        bitrate: u32,
    ) -> Result<Self> {
        let transform: IMFTransform = unsafe { activate.ActivateObject()? };
        let attributes = unsafe { transform.GetAttributes()? };
        if unsafe { attributes.GetUINT32(&MF_TRANSFORM_ASYNC) }.unwrap_or(0) == 0 {
            return Err(Error::empty());
        }
        unsafe { attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)? };
        let mut token = 0;
        let mut manager = None;
        unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager)? };
        let manager = manager.ok_or_else(Error::empty)?;
        unsafe {
            manager.ResetDevice(device, token)?;
            transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;
        }
        if let Ok(codec) = transform.cast::<ICodecAPI>() {
            let set = |api, value: VARIANT| unsafe {
                let _ = codec.SetValue(api, &value);
            };
            set(
                &CODECAPI_AVEncCommonRateControlMode,
                VARIANT::from(eAVEncCommonRateControlMode_UnconstrainedVBR.0 as u32),
            );
            set(&CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(bitrate));
            set(
                &CODECAPI_AVEncMPVGOPSize,
                VARIANT::from(u32::from(frame_rate) * 2),
            );
            set(&CODECAPI_AVEncMPVDefaultBPictureCount, VARIANT::from(0_u32));
            set(&CODECAPI_AVLowLatencyMode, VARIANT::from(true));
        }
        let frame_size = (u64::from(width) << 32) | u64::from(height);
        let rate = (u64::from(frame_rate) << 32) | 1;
        let video_type = |subtype| -> Result<IMFMediaType> {
            unsafe {
                let media_type = MFCreateMediaType()?;
                media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                media_type.SetGUID(&MF_MT_SUBTYPE, subtype)?;
                media_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size)?;
                media_type.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
                media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
                media_type
                    .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                media_type.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT601.0 as u32)?;
                media_type.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
                media_type
                    .SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_SMPTE170M.0 as u32)?;
                media_type.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
                Ok(media_type)
            }
        };
        let mut input_ids = [0_u32];
        let mut output_ids = [0_u32];
        if unsafe { transform.GetStreamIDs(&mut input_ids, &mut output_ids) }.is_err() {
            input_ids = [0];
            output_ids = [0];
        }
        let output_type = video_type(&MFVideoFormat_H264)?;
        unsafe {
            output_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            output_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)?;
            transform.SetOutputType(output_ids[0], &output_type, 0)?;
            transform.SetInputType(input_ids[0], &video_type(&MFVideoFormat_NV12)?, 0)?;
        }
        let info = unsafe { transform.GetOutputStreamInfo(output_ids[0])? };
        let events: IMFMediaEventGenerator = transform.cast()?;
        unsafe {
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        }
        Ok(Self {
            activate,
            transform,
            events,
            _manager: manager,
            input_id: input_ids[0],
            output_id: output_ids[0],
            provides_samples: info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0,
            output_size: info.cbSize.max(1 << 20),
            need_input: 0,
        })
    }

    /// One encoded access unit, or `None` after a format change.
    fn output(&self) -> Result<Option<EncodedFrame>> {
        let sample = if self.provides_samples {
            None
        } else {
            unsafe {
                let sample = MFCreateSample()?;
                let buffer =
                    windows::Win32::Media::MediaFoundation::MFCreateMemoryBuffer(self.output_size)?;
                sample.AddBuffer(&buffer)?;
                Some(sample)
            }
        };
        let mut buffer = MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: self.output_id,
            pSample: ManuallyDrop::new(sample),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        };
        let mut status = 0;
        let result = unsafe {
            self.transform
                .ProcessOutput(0, std::slice::from_mut(&mut buffer), &mut status)
        };
        let sample = ManuallyDrop::into_inner(buffer.pSample);
        drop(ManuallyDrop::into_inner(buffer.pEvents));
        match result {
            Ok(()) => {}
            Err(error) if error.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                unsafe {
                    let media_type = self.transform.GetOutputAvailableType(self.output_id, 0)?;
                    self.transform
                        .SetOutputType(self.output_id, &media_type, 0)?;
                }
                return Ok(None);
            }
            Err(error) => return Err(error),
        }
        let sample: IMFSample = sample.ok_or_else(Error::empty)?;
        unsafe {
            let time = sample.GetSampleTime()?;
            let key = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
            let buffer = sample.ConvertToContiguousBuffer()?;
            let mut data = std::ptr::null_mut();
            let mut len = 0;
            buffer.Lock(&mut data, None, Some(&mut len))?;
            let bytes = std::slice::from_raw_parts(data, len as usize).to_vec();
            buffer.Unlock()?;
            Ok(Some(EncodedFrame {
                time_100ns: u64::try_from(time).unwrap_or(0),
                key,
                annex_b: bytes,
            }))
        }
    }

    /// The SPS and PPS from the output type, for encoders that do not
    /// repeat them in the bitstream.
    fn sequence_header(&self) -> Option<Vec<u8>> {
        unsafe {
            let media_type = self.transform.GetOutputCurrentType(self.output_id).ok()?;
            let size = media_type.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER).ok()?;
            let mut header = vec![0; size as usize];
            media_type
                .GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut header, None)
                .ok()?;
            Some(header)
        }
    }
}

impl Drop for Mft {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            let _ = self.activate.ShutdownObject();
            let _ = MFShutdown();
        }
    }
}

struct EncodedFrame {
    time_100ns: u64,
    key: bool,
    annex_b: Vec<u8>,
}

struct PendingSample {
    start_ticks: u64,
    is_sync: bool,
    bytes: mp4::Bytes,
}

/// The MP4 track, written like captures-video's `H264Mp4Writer`.
struct Mux {
    path: PathBuf,
    size: (u32, u32),
    frame_rate: u16,
    writer: Option<Mp4Writer<File>>,
    pending: Option<PendingSample>,
    encoded: u64,
}

impl Mux {
    fn new(path: &Path, size: (u32, u32), frame_rate: u16) -> Self {
        Self {
            path: path.to_path_buf(),
            size,
            frame_rate,
            writer: None,
            pending: None,
            encoded: 0,
        }
    }

    fn write(&mut self, frame: &EncodedFrame, mft: &Mft) -> Result<()> {
        let mut sps = None;
        let mut pps = None;
        let mut bytes = Vec::with_capacity(frame.annex_b.len() + 16);
        for nal in nal_units(&frame.annex_b) {
            match nal[0] & 0x1f {
                7 => sps = Some(nal.to_vec()),
                8 => pps = Some(nal.to_vec()),
                // Access unit delimiters carry nothing an MP4 sample needs.
                9 => {}
                _ => {
                    bytes.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    bytes.extend_from_slice(nal);
                }
            }
        }
        if bytes.is_empty() {
            return Ok(());
        }
        if self.writer.is_none() {
            if sps.is_none() || pps.is_none() {
                for nal in mft.sequence_header().iter().flat_map(|h| nal_units(h)) {
                    match nal[0] & 0x1f {
                        7 => sps = sps.or_else(|| Some(nal.to_vec())),
                        8 => pps = pps.or_else(|| Some(nal.to_vec())),
                        _ => {}
                    }
                }
            }
            let (Some(sps), Some(pps)) = (sps, pps) else {
                return Err(Error::empty());
            };
            self.writer = Some(self.create(sps, pps).map_err(|_| Error::empty())?);
        }
        let start_ticks = frame.time_100ns * u64::from(MP4_TIMESCALE) / 10_000_000;
        if let Some(pending) = self.pending.take() {
            self.write_pending(pending, start_ticks, 1)?;
        }
        self.pending = Some(PendingSample {
            start_ticks,
            is_sync: frame.key,
            bytes: bytes.into(),
        });
        self.encoded += 1;
        Ok(())
    }

    fn create(&self, sps: Vec<u8>, pps: Vec<u8>) -> std::result::Result<Mp4Writer<File>, String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let file = File::create(&self.path).map_err(|e| e.to_string())?;
        let config = Mp4Config {
            major_brand: FourCC::from(*b"isom"),
            minor_version: 512,
            compatible_brands: vec![
                FourCC::from(*b"isom"),
                FourCC::from(*b"iso2"),
                FourCC::from(*b"avc1"),
                FourCC::from(*b"mp41"),
            ],
            timescale: MP4_TIMESCALE,
        };
        let mut writer = Mp4Writer::write_start(file, &config).map_err(|e| e.to_string())?;
        writer
            .add_track(&TrackConfig {
                track_type: TrackType::Video,
                timescale: MP4_TIMESCALE,
                language: "und".to_owned(),
                media_conf: MediaConfig::AvcConfig(AvcConfig {
                    width: u16::try_from(self.size.0).map_err(|e| e.to_string())?,
                    height: u16::try_from(self.size.1).map_err(|e| e.to_string())?,
                    seq_param_set: sps,
                    pic_param_set: pps,
                }),
            })
            .map_err(|e| e.to_string())?;
        Ok(writer)
    }

    /// A sample lasts until the next one starts; only the last frame gets a
    /// `minimum` of one frame, so the track keeps the recording's timing.
    fn write_pending(
        &mut self,
        pending: PendingSample,
        end_ticks: u64,
        minimum: u64,
    ) -> Result<()> {
        let duration = u32::try_from(end_ticks.saturating_sub(pending.start_ticks).max(minimum))
            .unwrap_or(u32::MAX);
        let sample = Mp4Sample {
            start_time: pending.start_ticks,
            duration,
            rendering_offset: 0,
            is_sync: pending.is_sync,
            bytes: pending.bytes,
        };
        self.writer
            .as_mut()
            .ok_or_else(Error::empty)?
            .write_sample(1, &sample)
            .map_err(|_| Error::empty())
    }

    fn finish(&mut self, end_ms: u64) -> Result<()> {
        let Some(pending) = self.pending.take() else {
            let _ = fs::remove_file(&self.path);
            return Ok(());
        };
        self.write_pending(
            pending,
            end_ms.saturating_mul(u64::from(MP4_TIMESCALE)) / 1_000,
            u64::from(MP4_TIMESCALE) / u64::from(self.frame_rate.max(1)),
        )?;
        let mut writer = self.writer.take().ok_or_else(Error::empty)?;
        writer.write_end().map_err(|_| Error::empty())?;
        writer.into_writer().flush().map_err(|_| Error::empty())
    }
}

/// The NAL units of an Annex B byte stream, without start codes.
fn nal_units(stream: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 <= stream.len() {
        if stream[index] == 0 && stream[index + 1] == 0 && stream[index + 2] == 1 {
            starts.push(index + 3);
            index += 3;
        } else {
            index += 1;
        }
    }
    let ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .map(|start| start - 3)
        .chain(std::iter::once(stream.len()))
        .collect();
    starts
        .into_iter()
        .zip(ends)
        .filter_map(move |(start, end)| {
            // A four-byte start code leaves its leading zero on the previous unit.
            let mut end = end;
            while end > start && stream[end - 1] == 0 {
                end -= 1;
            }
            (end > start).then(|| &stream[start..end])
        })
}

#[cfg(test)]
mod tests {
    use super::nal_units;

    #[test]
    fn splits_annex_b_on_three_and_four_byte_start_codes() {
        let stream = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 0,
        ];
        let units: Vec<&[u8]> = nal_units(&stream).collect();
        assert_eq!(units, [&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 4][..]]);
    }
}
