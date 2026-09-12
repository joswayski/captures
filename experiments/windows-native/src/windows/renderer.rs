use captures_windows_native::{
    geometry::{Rect, cover},
    history::Artifact,
    settings::Settings,
    state::{AppState, Surface},
    theme::{Color, Palette},
};
use image::RgbaImage;
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct2D::{
                Common::{
                    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
                    D2D1_PIXEL_FORMAT,
                },
                D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET,
                D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
                D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
                D2D1_INTERPOLATION_MODE_LINEAR, D2D1CreateFactory, ID2D1Bitmap1,
                ID2D1DeviceContext, ID2D1Factory1, ID2D1SolidColorBrush,
            },
            Direct3D::{
                D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
                D3D_FEATURE_LEVEL_11_0,
            },
            Direct3D11::{
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice,
                ID3D11Device,
            },
            DirectComposition::{
                DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget,
                IDCompositionVisual,
            },
            DirectWrite::{
                DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                DWRITE_TEXT_ALIGNMENT_CENTER, DWriteCreateFactory, IDWriteFactory,
                IDWriteTextFormat,
            },
            Dxgi::{
                Common::{
                    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN,
                    DXGI_SAMPLE_DESC,
                },
                DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
                DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter,
                IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
            },
        },
    },
    core::{HSTRING, Interface, Result},
};
use windows_numerics::Vector2;

pub struct Renderer {
    target: ID2D1DeviceContext,
    swap_chain: IDXGISwapChain1,
    back_buffer: Option<ID2D1Bitmap1>,
    _d3d: ID3D11Device,
    _composition_device: IDCompositionDevice,
    _composition_target: IDCompositionTarget,
    _composition_root: IDCompositionVisual,
    _write: IDWriteFactory,
    body: IDWriteTextFormat,
    strong: IDWriteTextFormat,
    title: IDWriteTextFormat,
    image_key: Option<(usize, u32, u32)>,
    image: Option<ID2D1Bitmap1>,
    driver_name: &'static str,
}

pub struct Frame<'a> {
    pub state: &'a AppState,
    pub settings: &'a Settings,
    pub history: &'a [Artifact],
    pub recording_mode: captures_capture::CaptureMode,
    pub palette: Palette,
    pub width: f32,
    pub height: f32,
}

impl Renderer {
    pub fn new(hwnd: HWND, width: u32, height: u32, dpi: f32) -> Result<Self> {
        unsafe {
            let (d3d, driver_name) = match create_d3d_device(D3D_DRIVER_TYPE_HARDWARE) {
                Ok(device) => (device, "hardware"),
                Err(_) => (create_d3d_device(D3D_DRIVER_TYPE_WARP)?, "warp"),
            };
            let dxgi: IDXGIDevice = d3d.cast()?;
            let adapter = dxgi.GetAdapter()?;
            let factory: IDXGIFactory2 = adapter.GetParent()?;
            let descriptor = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap_chain = factory.CreateSwapChainForComposition(&d3d, &descriptor, None)?;

            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d_device = d2d_factory.CreateDevice(&dxgi)?;
            let target = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let back_buffer = bind_back_buffer(&target, &swap_chain, dpi)?;

            let composition_device: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let composition_target = composition_device.CreateTargetForHwnd(hwnd, true)?;
            let composition_root = composition_device.CreateVisual()?;
            composition_root.SetContent(&swap_chain)?;
            composition_target.SetRoot(&composition_root)?;
            composition_device.Commit()?;
            let write: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let body = text_format(&write, 13.0, false)?;
            let strong = text_format(&write, 13.0, true)?;
            let title = text_format(&write, 22.0, true)?;
            Ok(Self {
                target,
                swap_chain,
                back_buffer: Some(back_buffer),
                _d3d: d3d,
                _composition_device: composition_device,
                _composition_target: composition_target,
                _composition_root: composition_root,
                _write: write,
                body,
                strong,
                title,
                image_key: None,
                image: None,
                driver_name,
            })
        }
    }

    pub fn driver_name(&self) -> &'static str {
        self.driver_name
    }

    pub fn resize(&mut self, width: u32, height: u32, dpi: f32) -> Result<()> {
        unsafe {
            if width == 0 || height == 0 {
                return Ok(());
            }
            self.target.SetTarget(None);
            self.back_buffer = None;
            self.image = None;
            self.image_key = None;
            self.swap_chain.ResizeBuffers(
                0,
                width,
                height,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
            self.back_buffer = Some(bind_back_buffer(&self.target, &self.swap_chain, dpi)?);
            Ok(())
        }
    }

    pub fn draw(&mut self, frame: Frame<'_>) -> Result<()> {
        unsafe {
            let Frame {
                state,
                settings,
                history,
                recording_mode,
                palette,
                width,
                height,
            } = frame;
            self.target.BeginDraw();
            self.target.Clear(Some(&color(match state.surface {
                Surface::Preview => Color(0, 0, 0, 0),
                Surface::Overlay | Surface::RecordingHud => palette.glass,
                _ => palette.canvas,
            })));
            match state.surface {
                Surface::Menu => self.menu(palette, width, height)?,
                Surface::Overlay => self.overlay(state, palette, width, height)?,
                Surface::ScreenshotEditor => self.image_editor(state, palette, width, height)?,
                Surface::RecordingSelector => {
                    self.recording_selector(settings, recording_mode, palette, width, height)?
                }
                Surface::RecordingHud => self.recording_hud(state, palette, width, height)?,
                Surface::RecordingEditor => self.recording_editor(state, palette, width, height)?,
                Surface::Preview => self.preview(state, palette, width, height)?,
                Surface::History => self.history(history, palette, width, height)?,
                Surface::Preferences => self.preferences(settings, palette, width, height)?,
                Surface::DeleteConfirmation => self.confirmation(state, palette, width, height)?,
            }
            self.target.EndDraw(None, None)?;
            self.swap_chain.Present(1, DXGI_PRESENT(0)).ok()
        }
    }

    unsafe fn menu(&self, p: Palette, w: f32, _h: f32) -> Result<()> {
        unsafe {
            self.text(
                "Captures",
                Rect {
                    x: 20.0,
                    y: 16.0,
                    width: w - 40.0,
                    height: 30.0,
                },
                p.text,
                &self.title,
            );
            self.text(
                "Capture anything on your screen",
                Rect {
                    x: 20.0,
                    y: 48.0,
                    width: w - 40.0,
                    height: 24.0,
                },
                p.muted,
                &self.body,
            );
            self.card(
                Rect {
                    x: 16.0,
                    y: 82.0,
                    width: w - 32.0,
                    height: 64.0,
                },
                p,
                "Region screenshot",
                "Drag to select exactly what you need",
                "R",
            );
            self.card(
                Rect {
                    x: 16.0,
                    y: 154.0,
                    width: w - 32.0,
                    height: 64.0,
                },
                p,
                "Window screenshot",
                "Point at an app window",
                "W",
            );
            self.card(
                Rect {
                    x: 16.0,
                    y: 226.0,
                    width: w - 32.0,
                    height: 64.0,
                },
                p,
                "Display screenshot",
                "Capture the active display",
                "D",
            );
            self.card(
                Rect {
                    x: 16.0,
                    y: 302.0,
                    width: w - 32.0,
                    height: 64.0,
                },
                p,
                "Record",
                "Video, GIF, system audio and microphone",
                "●",
            );
            self.button(
                Rect {
                    x: 16.0,
                    y: 378.0,
                    width: (w - 40.0) / 2.0,
                    height: 34.0,
                },
                p.raised,
                p.text,
                "History",
            );
            self.button(
                Rect {
                    x: 24.0 + (w - 40.0) / 2.0,
                    y: 378.0,
                    width: (w - 40.0) / 2.0,
                    height: 34.0,
                },
                p.raised,
                p.text,
                "Preferences",
            );
            Ok(())
        }
    }

    unsafe fn overlay(&mut self, state: &AppState, p: Palette, w: f32, h: f32) -> Result<()> {
        unsafe {
            let Some(overlay) = state.overlay.as_ref() else {
                return Ok(());
            };
            self.bitmap(
                &overlay.frame.image,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: w,
                    height: h,
                },
            )?;
            let veil = self.brush(Color(15, 15, 18, 105))?;
            self.target.FillRectangle(
                &D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: w,
                    bottom: h,
                },
                &veil,
            );
            let selection = match overlay.mode {
                captures_capture::CaptureMode::Region => overlay.selection,
                captures_capture::CaptureMode::Window => overlay
                    .hovered_window
                    .and_then(|index| overlay.windows.get(index))
                    .map(|window| {
                        let s = overlay.frame.descriptor.scale_factor.max(1.0) as f32;
                        Rect {
                            x: (window.x - overlay.frame.descriptor.x) as f32 / s,
                            y: (window.y - overlay.frame.descriptor.y) as f32 / s,
                            width: window.width as f32 / s,
                            height: window.height as f32 / s,
                        }
                    }),
                captures_capture::CaptureMode::Display => Some(Rect {
                    x: 0.0,
                    y: 0.0,
                    width: w,
                    height: h,
                }),
            };
            if let Some(rect) = selection {
                self.bitmap_crop_clear(&overlay.frame.image, rect, w, h)?;
                let brush = self.brush(p.accent)?;
                self.target.DrawRectangle(&to_d2d(rect), &brush, 2.0, None);
                self.text(
                    &format!(
                        "{} × {}",
                        rect.width.round() as u32,
                        rect.height.round() as u32
                    ),
                    Rect {
                        x: rect.x,
                        y: (rect.y - 28.0).max(8.0),
                        width: 120.0,
                        height: 22.0,
                    },
                    Color(246, 246, 248, 255),
                    &self.strong,
                );
            }
            self.button(
                Rect {
                    x: w / 2.0 - 154.0,
                    y: h - 58.0,
                    width: 100.0,
                    height: 38.0,
                },
                p.glass,
                p.text,
                "Cancel",
            );
            self.button(
                Rect {
                    x: w / 2.0 - 46.0,
                    y: h - 58.0,
                    width: 200.0,
                    height: 38.0,
                },
                p.accent,
                Color(23, 24, 27, 255),
                if overlay.recording {
                    "Start recording"
                } else {
                    "Capture"
                },
            );
            Ok(())
        }
    }

    unsafe fn image_editor(&mut self, state: &AppState, p: Palette, w: f32, h: f32) -> Result<()> {
        unsafe {
            self.panel(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: w,
                    height: 56.0,
                },
                p.raised,
            );
            self.text(
                "Select  Crop  Text  Pen  Arrow  Line  Rect  Ellipse  Triangle  Diamond  Star",
                Rect {
                    x: 20.0,
                    y: 14.0,
                    width: w - 280.0,
                    height: 28.0,
                },
                p.text,
                &self.strong,
            );
            self.button(
                Rect {
                    x: w - 236.0,
                    y: 11.0,
                    width: 68.0,
                    height: 34.0,
                },
                p.raised,
                p.text,
                "Copy",
            );
            self.button(
                Rect {
                    x: w - 160.0,
                    y: 11.0,
                    width: 68.0,
                    height: 34.0,
                },
                p.raised,
                p.text,
                "Save",
            );
            self.button(
                Rect {
                    x: w - 84.0,
                    y: 11.0,
                    width: 68.0,
                    height: 34.0,
                },
                p.accent,
                Color(23, 24, 27, 255),
                "Done",
            );
            if let Some(document) = state.editor.as_ref()
                && let Ok(image) = document.render()
            {
                self.bitmap(
                    &image,
                    Rect {
                        x: 76.0,
                        y: 80.0,
                        width: w - 300.0,
                        height: h - 130.0,
                    },
                )?;
            }
            self.panel(
                Rect {
                    x: w - 208.0,
                    y: 72.0,
                    width: 192.0,
                    height: h - 88.0,
                },
                p.raised,
            );
            self.text(
                "Layers",
                Rect {
                    x: w - 190.0,
                    y: 88.0,
                    width: 160.0,
                    height: 24.0,
                },
                p.text,
                &self.strong,
            );
            self.text(
                "Original screenshot\nAnnotations",
                Rect {
                    x: w - 190.0,
                    y: 128.0,
                    width: 160.0,
                    height: 80.0,
                },
                p.muted,
                &self.body,
            );
            self.button(
                Rect {
                    x: w - 190.0,
                    y: 228.0,
                    width: 112.0,
                    height: 32.0,
                },
                p.field,
                p.text,
                &state.editor_color_hex,
            );
            let swatch = self.brush(Color(
                state.editor_color[0],
                state.editor_color[1],
                state.editor_color[2],
                state.editor_color[3],
            ))?;
            self.target.FillEllipse(
                &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                    point: Vector2 {
                        X: w - 48.0,
                        Y: 244.0,
                    },
                    radiusX: 13.0,
                    radiusY: 13.0,
                },
                &swatch,
            );
            Ok(())
        }
    }

    unsafe fn recording_selector(
        &self,
        settings: &Settings,
        selected: captures_capture::CaptureMode,
        p: Palette,
        w: f32,
        _h: f32,
    ) -> Result<()> {
        unsafe {
            self.text(
                "Record your screen",
                Rect {
                    x: 24.0,
                    y: 22.0,
                    width: w - 48.0,
                    height: 32.0,
                },
                p.text,
                &self.title,
            );
            for (i, (a, b)) in [
                ("Region", "Select an area"),
                ("Window", "Choose an app"),
                ("Display", "Record a display"),
            ]
            .iter()
            .enumerate()
            {
                self.card(
                    Rect {
                        x: 20.0,
                        y: 78.0 + i as f32 * 72.0,
                        width: w - 40.0,
                        height: 62.0,
                    },
                    p,
                    a,
                    b,
                    "○",
                );
                if selected
                    == [
                        captures_capture::CaptureMode::Region,
                        captures_capture::CaptureMode::Window,
                        captures_capture::CaptureMode::Display,
                    ][i]
                {
                    let brush = self.brush(p.accent)?;
                    self.target.DrawRoundedRectangle(
                        &windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
                            rect: to_d2d(Rect {
                                x: 20.0,
                                y: 78.0 + i as f32 * 72.0,
                                width: w - 40.0,
                                height: 62.0,
                            }),
                            radiusX: 10.0,
                            radiusY: 10.0,
                        },
                        &brush,
                        2.0,
                        None,
                    );
                }
            }
            self.text(
                "Video · 60 FPS · Original",
                Rect {
                    x: 24.0,
                    y: 302.0,
                    width: w - 48.0,
                    height: 22.0,
                },
                p.muted,
                &self.body,
            );
            for (index, (label, enabled)) in [
                ("Cursor", settings.recording.show_cursor),
                ("Clicks", settings.recording.highlight_clicks),
                ("Keys", settings.recording.show_keystrokes),
                ("Audio", settings.recording.capture_system_audio),
            ]
            .iter()
            .enumerate()
            {
                let x = 28.0 + index as f32 * 104.0;
                self.text(
                    label,
                    Rect {
                        x,
                        y: 334.0,
                        width: 60.0,
                        height: 22.0,
                    },
                    p.text,
                    &self.body,
                );
                self.toggle(
                    Rect {
                        x: x + 62.0,
                        y: 337.0,
                        width: 32.0,
                        height: 16.0,
                    },
                    p,
                    *enabled,
                )?;
            }
            self.button(
                Rect {
                    x: 20.0,
                    y: 382.0,
                    width: w - 40.0,
                    height: 40.0,
                },
                p.accent,
                Color(23, 24, 27, 255),
                "Select area to record",
            );
            Ok(())
        }
    }

    unsafe fn recording_hud(&self, state: &AppState, p: Palette, w: f32, _h: f32) -> Result<()> {
        unsafe {
            let (time, label) =
                state
                    .recording
                    .as_ref()
                    .map_or(("00:00".into(), "PREPARING"), |r| {
                        let s = r.elapsed(std::time::Instant::now()).as_secs();
                        (
                            format!("{:02}:{:02}", s / 60, s % 60),
                            if r.state == captures_recording::RecordingState::Paused {
                                "PAUSED"
                            } else {
                                "RECORDING"
                            },
                        )
                    });
            self.text(
                "Controls are excluded from captures",
                Rect {
                    x: 8.0,
                    y: 4.0,
                    width: w - 16.0,
                    height: 14.0,
                },
                Color(246, 246, 248, 108),
                &self.body,
            );
            let dot = self.brush(if label == "PAUSED" {
                p.accent
            } else {
                p.signal
            })?;
            self.target.FillEllipse(
                &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                    point: Vector2 { X: 23.0, Y: 47.0 },
                    radiusX: 5.0,
                    radiusY: 5.0,
                },
                &dot,
            );
            self.text(
                &time,
                Rect {
                    x: 38.0,
                    y: 24.0,
                    width: 72.0,
                    height: 23.0,
                },
                Color(246, 246, 248, 255),
                &self.strong,
            );
            self.text(
                label,
                Rect {
                    x: 38.0,
                    y: 47.0,
                    width: 72.0,
                    height: 15.0,
                },
                Color(246, 246, 248, 108),
                &self.body,
            );
            for (i, icon) in [
                "stop",
                "pause",
                "restart",
                "capture",
                "microphone",
                "trash",
                "hide",
            ]
            .iter()
            .enumerate()
            {
                let rect = Rect {
                    x: 126.0 + i as f32 * 52.0,
                    y: 27.0,
                    width: 44.0,
                    height: 36.0,
                };
                self.button(
                    rect,
                    if i == 0 {
                        Color(p.signal.0, p.signal.1, p.signal.2, 45)
                    } else {
                        Color(255, 255, 255, 14)
                    },
                    Color(246, 246, 248, 200),
                    "",
                );
                self.hud_icon(
                    icon,
                    rect,
                    if i == 0 {
                        p.signal
                    } else {
                        Color(246, 246, 248, 200)
                    },
                )?;
            }
            Ok(())
        }
    }

    unsafe fn recording_editor(
        &mut self,
        state: &AppState,
        p: Palette,
        w: f32,
        h: f32,
    ) -> Result<()> {
        unsafe {
            let editor = state.recording_editor.as_ref();
            self.text(
                "Recording editor",
                Rect {
                    x: 24.0,
                    y: 18.0,
                    width: 220.0,
                    height: 32.0,
                },
                p.text,
                &self.title,
            );
            self.button(
                Rect {
                    x: w - 112.0,
                    y: 16.0,
                    width: 88.0,
                    height: 36.0,
                },
                p.accent,
                Color(23, 24, 27, 255),
                "Export",
            );
            self.panel(
                Rect {
                    x: 24.0,
                    y: 74.0,
                    width: w - 280.0,
                    height: h - 220.0,
                },
                Color(11, 11, 14, 255),
            );
            self.panel(
                Rect {
                    x: w - 236.0,
                    y: 74.0,
                    width: 212.0,
                    height: h - 220.0,
                },
                p.raised,
            );
            if let Some(preview) = &state.recording_preview {
                self.bitmap(
                    preview,
                    Rect {
                        x: 82.0,
                        y: 126.0,
                        width: w - 342.0,
                        height: h - 286.0,
                    },
                )?;
            }
            self.text("Crop & size\nOriginal resolution\n\nAudio\nPreserved on export\nPlayback unavailable\n\nFormat",Rect{x:w-216.0,y:94.0,width:172.0,height:200.0},p.muted,&self.body);
            self.button(
                Rect {
                    x: w - 216.0,
                    y: 286.0,
                    width: 172.0,
                    height: 34.0,
                },
                p.field,
                p.text,
                match editor.map(|editor| editor.quality) {
                    Some(captures_media::QualityPreset::Preserve) | None => "Preserve quality  ▾",
                    Some(captures_media::QualityPreset::Highest) => "Highest quality   ▾",
                    Some(captures_media::QualityPreset::High) => "High quality      ▾",
                    Some(captures_media::QualityPreset::Standard) => "Standard quality  ▾",
                    Some(captures_media::QualityPreset::Small) => "Small file        ▾",
                    Some(captures_media::QualityPreset::Tiny) => "Tiny file         ▾",
                },
            );
            self.text(
                &editor
                    .and_then(|editor| editor.comparison_estimated_bytes)
                    .map_or_else(
                        || "Building compression comparison…".to_owned(),
                        |bytes| format!("Before  |  After     Estimated {}", format_bytes(bytes)),
                    ),
                Rect {
                    x: w - 216.0,
                    y: 330.0,
                    width: 172.0,
                    height: 44.0,
                },
                p.muted,
                &self.body,
            );
            self.text(
                "Save as new file",
                Rect {
                    x: w - 216.0,
                    y: 386.0,
                    width: 126.0,
                    height: 24.0,
                },
                p.text,
                &self.body,
            );
            self.toggle(
                Rect {
                    x: w - 78.0,
                    y: 390.0,
                    width: 34.0,
                    height: 18.0,
                },
                p,
                editor.is_none_or(|editor| editor.save_as_new),
            )?;
            if editor.is_some_and(|editor| editor.quality_menu_open) {
                self.panel(
                    Rect {
                        x: w - 216.0,
                        y: 320.0,
                        width: 172.0,
                        height: 180.0,
                    },
                    p.field,
                );
                for (index, label) in ["Preserve", "Highest", "High", "Standard", "Small", "Tiny"]
                    .iter()
                    .enumerate()
                {
                    self.text(
                        label,
                        Rect {
                            x: w - 202.0,
                            y: 324.0 + index as f32 * 30.0,
                            width: 144.0,
                            height: 24.0,
                        },
                        p.text,
                        &self.body,
                    );
                }
            }
            self.panel(
                Rect {
                    x: 24.0,
                    y: h - 124.0,
                    width: w - 48.0,
                    height: 84.0,
                },
                p.raised,
            );
            self.text(
                &editor.map_or_else(
                    || "▶  00:00   ├━━━━━━━━━━━━━━━━━━━━┤   End".to_owned(),
                    |editor| {
                        format!(
                            "{}  {}   ├━━━━━━━━━━━━━━━━━━━━┤   {}",
                            if editor.playing { "Ⅱ" } else { "▶" },
                            format_time(editor.position_ms),
                            format_time(editor.duration_ms),
                        )
                    },
                ),
                Rect {
                    x: 44.0,
                    y: h - 94.0,
                    width: w - 88.0,
                    height: 28.0,
                },
                p.text,
                &self.body,
            );
            Ok(())
        }
    }

    unsafe fn preview(&mut self, state: &AppState, p: Palette, w: f32, _h: f32) -> Result<()> {
        unsafe {
            let count = state.previews.len().min(5);
            for index in (0..count).rev() {
                let preview = &state.previews[index];
                let y = (count - 1 - index) as f32 * 28.0;
                let media = Rect {
                    x: 0.0,
                    y,
                    width: w,
                    height: 162.0,
                };
                if let Some(start) = preview.dismissing {
                    self.bitmap_fragments(
                        &preview.image,
                        media,
                        start.elapsed().as_secs_f32() / 0.42,
                    )?;
                } else {
                    self.bitmap(&preview.image, media)?;
                }
                self.panel(
                    Rect {
                        x: 0.0,
                        y: y + 162.0,
                        width: w,
                        height: 38.0,
                    },
                    p.glass,
                );
                self.text(
                    "Edit        Copy        Drag        Delete",
                    Rect {
                        x: 12.0,
                        y: y + 169.0,
                        width: w - 24.0,
                        height: 24.0,
                    },
                    Color(246, 246, 248, 220),
                    &self.strong,
                );
            }
            Ok(())
        }
    }
    unsafe fn history(&self, history: &[Artifact], p: Palette, w: f32, _h: f32) -> Result<()> {
        unsafe {
            self.text(
                "Capture History",
                Rect {
                    x: 28.0,
                    y: 22.0,
                    width: w - 56.0,
                    height: 32.0,
                },
                p.text,
                &self.title,
            );
            self.text(
                "Your recent screenshots, GIFs and videos",
                Rect {
                    x: 28.0,
                    y: 56.0,
                    width: w - 56.0,
                    height: 24.0,
                },
                p.muted,
                &self.body,
            );
            for (i, artifact) in history.iter().take(3).enumerate() {
                self.panel(
                    Rect {
                        x: 28.0 + i as f32 * 210.0,
                        y: 104.0,
                        width: 190.0,
                        height: 150.0,
                    },
                    p.raised,
                );
                self.text(
                    artifact
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Capture"),
                    Rect {
                        x: 36.0 + i as f32 * 210.0,
                        y: 126.0,
                        width: 174.0,
                        height: 42.0,
                    },
                    p.text,
                    &self.strong,
                );
                self.text(
                    if artifact.is_trashed() {
                        "In Trash"
                    } else {
                        "Saved"
                    },
                    Rect {
                        x: 36.0 + i as f32 * 210.0,
                        y: 174.0,
                        width: 174.0,
                        height: 24.0,
                    },
                    if artifact.is_trashed() {
                        p.signal
                    } else {
                        p.muted
                    },
                    &self.body,
                );
                let x = 36.0 + i as f32 * 210.0;
                for (button, label) in ["Edit", "Restore", "Delete"].iter().enumerate() {
                    let enabled = match button {
                        0 => !artifact.is_trashed(),
                        1 => artifact.is_trashed(),
                        _ => true,
                    };
                    self.button(
                        Rect {
                            x: x + button as f32 * 57.0,
                            y: 218.0,
                            width: 54.0,
                            height: 28.0,
                        },
                        if enabled { p.field } else { p.canvas },
                        if !enabled {
                            p.muted
                        } else if *label == "Delete" {
                            p.signal
                        } else {
                            p.text
                        },
                        label,
                    );
                }
            }
            self.text(
                "Open, edit, copy, drag, or remove a capture",
                Rect {
                    x: 28.0,
                    y: 278.0,
                    width: w - 56.0,
                    height: 24.0,
                },
                p.muted,
                &self.body,
            );
            Ok(())
        }
    }
    unsafe fn preferences(&self, settings: &Settings, p: Palette, w: f32, h: f32) -> Result<()> {
        unsafe {
            self.panel(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 184.0,
                    height: h,
                },
                p.raised,
            );
            self.text(
                "Captures",
                Rect {
                    x: 22.0,
                    y: 22.0,
                    width: 142.0,
                    height: 30.0,
                },
                p.text,
                &self.title,
            );
            self.text(
                "General\n\nCapture\n\nRecording\n\nShortcuts\n\nAppearance",
                Rect {
                    x: 22.0,
                    y: 82.0,
                    width: 142.0,
                    height: 260.0,
                },
                p.muted,
                &self.strong,
            );
            self.text(
                "Preferences",
                Rect {
                    x: 216.0,
                    y: 22.0,
                    width: w - 244.0,
                    height: 32.0,
                },
                p.text,
                &self.title,
            );
            let rows = [
                (
                    "Start Captures at login",
                    Some(settings.launch_at_login),
                    "",
                ),
                (
                    "Copy captures automatically",
                    Some(settings.auto_copy_to_clipboard),
                    "",
                ),
                ("Show mini previews", Some(settings.show_mini_previews), ""),
                (
                    "Freeze screen while selecting",
                    Some(settings.freeze_screen),
                    "",
                ),
                ("Appearance", None, settings.appearance.as_str()),
                ("Color theme", None, settings.theme.as_str()),
            ];
            for (i, (label, toggle, value)) in rows.iter().enumerate() {
                let y = 88.0 + i as f32 * 64.0;
                self.text(
                    label,
                    Rect {
                        x: 216.0,
                        y,
                        width: w - 390.0,
                        height: 28.0,
                    },
                    p.text,
                    &self.strong,
                );
                if let Some(enabled) = toggle {
                    self.toggle(
                        Rect {
                            x: w - 76.0,
                            y: y + 2.0,
                            width: 36.0,
                            height: 18.0,
                        },
                        p,
                        *enabled,
                    )?;
                } else {
                    self.button(
                        Rect {
                            x: w - 152.0,
                            y: y - 4.0,
                            width: 120.0,
                            height: 32.0,
                        },
                        p.field,
                        p.text,
                        value,
                    );
                }
            }
            Ok(())
        }
    }
    unsafe fn confirmation(&self, state: &AppState, p: Palette, w: f32, h: f32) -> Result<()> {
        unsafe {
            self.panel(
                Rect {
                    x: w / 2.0 - 180.0,
                    y: h / 2.0 - 96.0,
                    width: 360.0,
                    height: 192.0,
                },
                p.raised,
            );
            let permanent = state
                .pending_delete
                .as_ref()
                .is_some_and(Artifact::is_trashed);
            self.text(
                if permanent {
                    "Delete permanently?"
                } else {
                    "Move capture to Trash?"
                },
                Rect {
                    x: w / 2.0 - 152.0,
                    y: h / 2.0 - 67.0,
                    width: 304.0,
                    height: 30.0,
                },
                p.text,
                &self.title,
            );
            let name = state
                .pending_delete
                .as_ref()
                .and_then(|a| a.path.file_name())
                .and_then(|v| v.to_str())
                .unwrap_or("This file");
            self.text(
                &format!(
                    "{name} will {}.",
                    if permanent {
                        "be permanently removed"
                    } else {
                        "remain available to restore"
                    }
                ),
                Rect {
                    x: w / 2.0 - 152.0,
                    y: h / 2.0 - 27.0,
                    width: 304.0,
                    height: 42.0,
                },
                p.muted,
                &self.body,
            );
            self.button(
                Rect {
                    x: w / 2.0 - 152.0,
                    y: h / 2.0 + 37.0,
                    width: 140.0,
                    height: 36.0,
                },
                p.field,
                p.text,
                "Cancel",
            );
            self.button(
                Rect {
                    x: w / 2.0 + 12.0,
                    y: h / 2.0 + 37.0,
                    width: 140.0,
                    height: 36.0,
                },
                p.signal,
                Color(255, 255, 255, 255),
                if permanent { "Delete" } else { "Move to Trash" },
            );
            Ok(())
        }
    }

    unsafe fn card(&self, rect: Rect, p: Palette, title: &str, description: &str, icon: &str) {
        unsafe {
            self.panel(rect, p.raised);
            self.button(
                Rect {
                    x: rect.x + 12.0,
                    y: rect.y + 12.0,
                    width: 40.0,
                    height: 40.0,
                },
                Color(p.accent.0, p.accent.1, p.accent.2, 42),
                p.accent,
                icon,
            );
            self.text(
                title,
                Rect {
                    x: rect.x + 64.0,
                    y: rect.y + 10.0,
                    width: rect.width - 76.0,
                    height: 24.0,
                },
                p.text,
                &self.strong,
            );
            self.text(
                description,
                Rect {
                    x: rect.x + 64.0,
                    y: rect.y + 33.0,
                    width: rect.width - 76.0,
                    height: 20.0,
                },
                p.muted,
                &self.body,
            )
        }
    }
    unsafe fn panel(&self, rect: Rect, fill: Color) {
        unsafe {
            let brush = self.brush(fill).expect("brush");
            self.target.FillRectangle(&to_d2d(rect), &brush)
        }
    }
    unsafe fn button(&self, rect: Rect, fill: Color, ink: Color, label: &str) {
        unsafe {
            self.panel(rect, fill);
            self.text(label, rect.inset(4.0), ink, &self.strong)
        }
    }
    unsafe fn text(&self, value: &str, rect: Rect, ink: Color, format: &IDWriteTextFormat) {
        unsafe {
            if let Ok(brush) = self.brush(ink) {
                let wide = value.encode_utf16().collect::<Vec<_>>();
                self.target.DrawText(
                    &wide,
                    format,
                    &to_d2d(rect),
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                )
            }
        }
    }
    unsafe fn brush(&self, value: Color) -> Result<ID2D1SolidColorBrush> {
        unsafe { self.target.CreateSolidColorBrush(&color(value), None) }
    }

    unsafe fn toggle(&self, rect: Rect, p: Palette, enabled: bool) -> Result<()> {
        unsafe {
            let track = self.brush(if enabled { p.accent } else { p.border })?;
            self.target.FillRoundedRectangle(
                &windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
                    rect: to_d2d(rect),
                    radiusX: rect.height / 2.0,
                    radiusY: rect.height / 2.0,
                },
                &track,
            );
            let knob = self.brush(if enabled {
                Color(23, 24, 27, 255)
            } else {
                p.muted
            })?;
            self.target.FillEllipse(
                &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                    point: Vector2 {
                        X: if enabled {
                            rect.x + rect.width - rect.height / 2.0
                        } else {
                            rect.x + rect.height / 2.0
                        },
                        Y: rect.y + rect.height / 2.0,
                    },
                    radiusX: rect.height / 2.0 - 2.0,
                    radiusY: rect.height / 2.0 - 2.0,
                },
                &knob,
            );
            Ok(())
        }
    }

    unsafe fn hud_icon(&self, name: &str, rect: Rect, ink: Color) -> Result<()> {
        unsafe {
            let brush = self.brush(ink)?;
            let c = Vector2 {
                X: rect.x + rect.width / 2.0,
                Y: rect.y + rect.height / 2.0,
            };
            let line = |a: Vector2, b: Vector2| self.target.DrawLine(a, b, &brush, 1.8, None);
            match name {
                "stop" => self.target.FillRectangle(
                    &D2D_RECT_F {
                        left: c.X - 5.0,
                        top: c.Y - 5.0,
                        right: c.X + 5.0,
                        bottom: c.Y + 5.0,
                    },
                    &brush,
                ),
                "pause" => {
                    line(
                        Vector2 {
                            X: c.X - 4.0,
                            Y: c.Y - 6.0,
                        },
                        Vector2 {
                            X: c.X - 4.0,
                            Y: c.Y + 6.0,
                        },
                    );
                    line(
                        Vector2 {
                            X: c.X + 4.0,
                            Y: c.Y - 6.0,
                        },
                        Vector2 {
                            X: c.X + 4.0,
                            Y: c.Y + 6.0,
                        },
                    );
                }
                "restart" => {
                    self.target.DrawEllipse(
                        &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                            point: c,
                            radiusX: 7.0,
                            radiusY: 7.0,
                        },
                        &brush,
                        1.8,
                        None,
                    );
                    line(
                        Vector2 {
                            X: c.X - 8.0,
                            Y: c.Y - 5.0,
                        },
                        Vector2 {
                            X: c.X - 8.0,
                            Y: c.Y + 1.0,
                        },
                    );
                }
                "capture" => {
                    for (a, b) in [
                        ((-7.0, -3.0), (-7.0, -7.0)),
                        ((-7.0, -7.0), (-3.0, -7.0)),
                        ((7.0, 3.0), (7.0, 7.0)),
                        ((7.0, 7.0), (3.0, 7.0)),
                    ] {
                        line(
                            Vector2 {
                                X: c.X + a.0,
                                Y: c.Y + a.1,
                            },
                            Vector2 {
                                X: c.X + b.0,
                                Y: c.Y + b.1,
                            },
                        );
                    }
                }
                "microphone" => {
                    line(
                        Vector2 {
                            X: c.X,
                            Y: c.Y - 7.0,
                        },
                        Vector2 {
                            X: c.X,
                            Y: c.Y + 3.0,
                        },
                    );
                    line(
                        Vector2 {
                            X: c.X - 5.0,
                            Y: c.Y + 1.0,
                        },
                        Vector2 {
                            X: c.X,
                            Y: c.Y + 7.0,
                        },
                    );
                    line(
                        Vector2 {
                            X: c.X,
                            Y: c.Y + 7.0,
                        },
                        Vector2 {
                            X: c.X + 5.0,
                            Y: c.Y + 1.0,
                        },
                    );
                }
                "trash" => {
                    self.target.DrawRectangle(
                        &D2D_RECT_F {
                            left: c.X - 5.0,
                            top: c.Y - 4.0,
                            right: c.X + 5.0,
                            bottom: c.Y + 7.0,
                        },
                        &brush,
                        1.8,
                        None,
                    );
                    line(
                        Vector2 {
                            X: c.X - 7.0,
                            Y: c.Y - 6.0,
                        },
                        Vector2 {
                            X: c.X + 7.0,
                            Y: c.Y - 6.0,
                        },
                    );
                }
                _ => {
                    line(
                        Vector2 {
                            X: c.X - 7.0,
                            Y: c.Y - 7.0,
                        },
                        Vector2 {
                            X: c.X + 7.0,
                            Y: c.Y + 7.0,
                        },
                    );
                    line(
                        Vector2 {
                            X: c.X + 7.0,
                            Y: c.Y - 7.0,
                        },
                        Vector2 {
                            X: c.X - 7.0,
                            Y: c.Y + 7.0,
                        },
                    );
                }
            }
            Ok(())
        }
    }

    unsafe fn bitmap(&mut self, image: &RgbaImage, destination: Rect) -> Result<()> {
        unsafe {
            self.ensure_bitmap(image)?;
            let fitted = cover((image.width(), image.height()), destination);
            self.target.DrawBitmap(
                self.image.as_ref().unwrap(),
                Some(&to_d2d(fitted)),
                1.0,
                D2D1_INTERPOLATION_MODE_LINEAR,
                None,
                None,
            );
            Ok(())
        }
    }

    unsafe fn ensure_bitmap(&mut self, image: &RgbaImage) -> Result<()> {
        unsafe {
            let key = (image.as_ptr() as usize, image.width(), image.height());
            if self.image_key == Some(key) {
                return Ok(());
            }
            let bytes = bgra(image);
            let properties = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            self.image = Some(self.target.CreateBitmap(
                D2D_SIZE_U {
                    width: image.width(),
                    height: image.height(),
                },
                Some(bytes.as_ptr().cast()),
                image.width() * 4,
                &properties,
            )?);
            self.image_key = Some(key);
            Ok(())
        }
    }

    unsafe fn bitmap_fragments(
        &mut self,
        image: &RgbaImage,
        destination: Rect,
        progress: f32,
    ) -> Result<()> {
        unsafe {
            self.ensure_bitmap(image)?;
            let progress = progress.clamp(0.0, 1.0);
            let fitted = cover((image.width(), image.height()), destination);
            for row in 0..5 {
                for column in 0..8 {
                    let source = D2D_RECT_F {
                        left: image.width() as f32 * column as f32 / 8.0,
                        top: image.height() as f32 * row as f32 / 5.0,
                        right: image.width() as f32 * (column + 1) as f32 / 8.0,
                        bottom: image.height() as f32 * (row + 1) as f32 / 5.0,
                    };
                    let phase = (column as f32 * 0.071 + row as f32 * 0.113) % 0.28;
                    let local = ((progress - phase) / 0.72).clamp(0.0, 1.0);
                    let fragment = Rect {
                        x: fitted.x
                            + fitted.width * column as f32 / 8.0
                            + (column as f32 - 3.5) * local * 7.0,
                        y: fitted.y + fitted.height * row as f32 / 5.0
                            - local * (18.0 + row as f32 * 5.0),
                        width: fitted.width / 8.0 + 0.5,
                        height: fitted.height / 5.0 + 0.5,
                    };
                    self.target.DrawBitmap(
                        self.image.as_ref().unwrap(),
                        Some(&to_d2d(fragment)),
                        1.0 - local,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        Some(&source),
                        None,
                    );
                }
            }
            Ok(())
        }
    }
    unsafe fn bitmap_crop_clear(
        &mut self,
        image: &RgbaImage,
        rect: Rect,
        w: f32,
        h: f32,
    ) -> Result<()> {
        unsafe {
            let key = (image.as_ptr() as usize, image.width(), image.height());
            if self.image_key != Some(key) {
                self.bitmap(
                    image,
                    Rect {
                        x: 0.0,
                        y: 0.0,
                        width: w,
                        height: h,
                    },
                )?;
            }
            let source = to_d2d(rect);
            self.target.DrawBitmap(
                self.image.as_ref().unwrap(),
                Some(&source),
                1.0,
                D2D1_INTERPOLATION_MODE_LINEAR,
                Some(&source),
                None,
            );
            Ok(())
        }
    }
}

unsafe fn create_d3d_device(driver: D3D_DRIVER_TYPE) -> Result<ID3D11Device> {
    unsafe {
        let mut device = None;
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
        device.ok_or_else(|| {
            windows::core::Error::from_hresult(windows::core::HRESULT(0x80004005_u32 as i32))
        })
    }
}

fn format_time(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{} KB", bytes.div_ceil(1_000))
    }
}

fn text_format(factory: &IDWriteFactory, size: f32, strong: bool) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = factory.CreateTextFormat(
            &HSTRING::from("Segoe UI Variable Text"),
            None,
            if strong {
                DWRITE_FONT_WEIGHT_SEMI_BOLD
            } else {
                DWRITE_FONT_WEIGHT_NORMAL
            },
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            &HSTRING::from("en-US"),
        )?;
        format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        Ok(format)
    }
}
fn to_d2d(rect: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.x,
        top: rect.y,
        right: rect.x + rect.width,
        bottom: rect.y + rect.height,
    }
}
unsafe fn bind_back_buffer(
    target: &ID2D1DeviceContext,
    swap_chain: &IDXGISwapChain1,
    dpi: f32,
) -> Result<ID2D1Bitmap1> {
    unsafe {
        let surface: IDXGISurface = swap_chain.GetBuffer(0)?;
        let properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: dpi,
            dpiY: dpi,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            ..Default::default()
        };
        let bitmap = target.CreateBitmapFromDxgiSurface(&surface, Some(&properties))?;
        target.SetTarget(&bitmap);
        target.SetDpi(dpi, dpi);
        Ok(bitmap)
    }
}
fn color(value: Color) -> D2D1_COLOR_F {
    let [r, g, b, a] = value.rgba();
    D2D1_COLOR_F { r, g, b, a }
}
fn bgra(image: &RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(image.len());
    for pixel in image.pixels() {
        let a = u16::from(pixel[3]);
        bytes.extend([
            ((u16::from(pixel[2]) * a) / 255) as u8,
            ((u16::from(pixel[1]) * a) / 255) as u8,
            ((u16::from(pixel[0]) * a) / 255) as u8,
            pixel[3],
        ])
    }
    bytes
}
