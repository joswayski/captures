use captures_windows_native::{
    editor::{RemoveBackgroundMode, Tool},
    geometry::{
        Rect, contain, cover, editor_layer_lock_button, editor_layer_visibility_button,
        editor_shape_flyout_cell, recording_editor_timeline_track, screenshot_editor_canvas,
        screenshot_editor_viewport,
    },
    history::Artifact,
    settings::Settings,
    state::{AppState, PreferencesPage, Surface, can_replace_editor_source},
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
                D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
                D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE,
                D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1CreateFactory, ID2D1Bitmap1, ID2D1DeviceContext, ID2D1Factory1,
                ID2D1SolidColorBrush,
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
                Surface::Preferences => {
                    self.preferences(state, settings, palette, width, height)?
                }
                Surface::Feedback => self.feedback(state, palette, width, height)?,
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
                "select",
            )?;
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
                "window",
            )?;
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
                "display",
            )?;
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
                "video",
            )?;
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
            let footer_y = h - 92.0;
            let sidebar_x = w - 320.0;
            self.panel(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: w,
                    height: 52.0,
                },
                p.raised,
            );
            self.panel(
                Rect {
                    x: 0.0,
                    y: 52.0,
                    width: 56.0,
                    height: footer_y - 52.0,
                },
                p.raised,
            );
            self.panel(
                Rect {
                    x: sidebar_x,
                    y: 52.0,
                    width: 320.0,
                    height: footer_y - 52.0,
                },
                p.raised,
            );
            self.panel(
                Rect {
                    x: 0.0,
                    y: footer_y,
                    width: w,
                    height: 92.0,
                },
                p.raised,
            );
            let border = self.brush(p.border)?;
            self.target.DrawLine(
                Vector2 { X: 0.0, Y: 52.0 },
                Vector2 { X: w, Y: 52.0 },
                &border,
                1.0,
                None,
            );
            self.target.DrawLine(
                Vector2 {
                    X: sidebar_x,
                    Y: 52.0,
                },
                Vector2 {
                    X: sidebar_x,
                    Y: footer_y,
                },
                &border,
                1.0,
                None,
            );
            self.target.DrawLine(
                Vector2 {
                    X: 0.0,
                    Y: footer_y,
                },
                Vector2 { X: w, Y: footer_y },
                &border,
                1.0,
                None,
            );

            let document = state.editor.as_ref();
            let dimensions = document.map_or_else(
                || "W   —       ×       H   —".to_owned(),
                |document| {
                    format!(
                        "W   {}       ×       H   {}",
                        if state.editor_input_field
                            == Some(captures_windows_native::state::EditorInputField::CanvasWidth)
                        {
                            state.editor_input_text.clone()
                        } else {
                            document.canvas_width.to_string()
                        },
                        if state.editor_input_field
                            == Some(captures_windows_native::state::EditorInputField::CanvasHeight)
                        {
                            state.editor_input_text.clone()
                        } else {
                            document.canvas_height.to_string()
                        }
                    )
                },
            );
            self.rounded_panel(
                Rect {
                    x: 16.0,
                    y: 9.0,
                    width: 410.0,
                    height: 34.0,
                },
                p.field,
                8.0,
            )?;
            self.text(
                "Canvas",
                Rect {
                    x: 26.0,
                    y: 13.0,
                    width: 52.0,
                    height: 26.0,
                },
                p.muted,
                &self.body,
            );
            self.text(
                &dimensions,
                Rect {
                    x: 78.0,
                    y: 13.0,
                    width: 148.0,
                    height: 26.0,
                },
                p.text,
                &self.body,
            );
            self.divider(
                Vector2 { X: 232.0, Y: 18.0 },
                Vector2 { X: 232.0, Y: 34.0 },
                p.border,
            )?;
            let trim_ink =
                if document.is_some_and(|document| document.can_trim_to_visible_content()) {
                    p.text
                } else {
                    p.border
                };
            self.editor_icon(
                "crop",
                Rect {
                    x: 244.0,
                    y: 18.0,
                    width: 14.0,
                    height: 14.0,
                },
                trim_ink,
            )?;
            self.text(
                "Trim edges",
                Rect {
                    x: 262.0,
                    y: 13.0,
                    width: 74.0,
                    height: 26.0,
                },
                trim_ink,
                &self.body,
            );
            self.divider(
                Vector2 { X: 340.0, Y: 18.0 },
                Vector2 { X: 340.0, Y: 34.0 },
                p.border,
            )?;
            self.rounded_panel(
                Rect {
                    x: 352.0,
                    y: 19.0,
                    width: 14.0,
                    height: 14.0,
                },
                document
                    .and_then(|document| document.background)
                    .map_or(Color(255, 255, 255, 255), |color| {
                        Color(color[0], color[1], color[2], color[3])
                    }),
                3.0,
            )?;
            self.text(
                if document.is_some_and(|document| document.background.is_some()) {
                    "Solid"
                } else {
                    "Transparent"
                },
                Rect {
                    x: 370.0,
                    y: 13.0,
                    width: 82.0,
                    height: 26.0,
                },
                p.muted,
                &self.body,
            );
            for (index, icon) in ["undo", "redo"].iter().enumerate() {
                let rect = Rect {
                    x: w - 498.0 + index as f32 * 38.0,
                    y: 9.0,
                    width: 34.0,
                    height: 34.0,
                };
                self.editor_icon(icon, rect.inset(8.0), p.muted)?;
            }
            self.rounded_panel(
                Rect {
                    x: w - 414.0,
                    y: 9.0,
                    width: 190.0,
                    height: 34.0,
                },
                p.field,
                8.0,
            )?;
            self.editor_icon(
                "fit",
                Rect {
                    x: w - 400.0,
                    y: 19.0,
                    width: 14.0,
                    height: 14.0,
                },
                p.muted,
            )?;
            self.editor_icon(
                "minus",
                Rect {
                    x: w - 372.0,
                    y: 20.0,
                    width: 12.0,
                    height: 12.0,
                },
                p.muted,
            )?;
            self.divider(
                Vector2 {
                    X: w - 348.0,
                    Y: 26.0,
                },
                Vector2 {
                    X: w - 292.0,
                    Y: 26.0,
                },
                p.muted,
            )?;
            let zoom_knob = self.brush(p.muted)?;
            self.target.FillEllipse(
                &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                    point: Vector2 {
                        X: w - 320.0,
                        Y: 26.0,
                    },
                    radiusX: 3.5,
                    radiusY: 3.5,
                },
                &zoom_knob,
            );
            self.editor_icon(
                "plus",
                Rect {
                    x: w - 280.0,
                    y: 20.0,
                    width: 12.0,
                    height: 12.0,
                },
                p.muted,
            )?;
            let zoom_label = if state.editor_zoom_fit {
                "Fit".to_owned()
            } else {
                format!("{}%", state.editor_zoom_percent)
            };
            self.text(
                &zoom_label,
                Rect {
                    x: w - 258.0,
                    y: 13.0,
                    width: 30.0,
                    height: 26.0,
                },
                p.muted,
                &self.body,
            );
            self.rounded_panel(
                Rect {
                    x: w - 214.0,
                    y: 9.0,
                    width: 194.0,
                    height: 34.0,
                },
                p.field,
                7.0,
            )?;
            self.editor_icon(
                "image",
                Rect {
                    x: w - 196.0,
                    y: 18.0,
                    width: 16.0,
                    height: 16.0,
                },
                p.text,
            )?;
            self.text(
                "Add images",
                Rect {
                    x: w - 174.0,
                    y: 13.0,
                    width: 150.0,
                    height: 26.0,
                },
                p.text,
                &self.body,
            );

            if let (Some(document), Some(image)) = (document, state.editor_preview.as_ref()) {
                let viewport = screenshot_editor_viewport(
                    (document.canvas_width, document.canvas_height),
                    screenshot_editor_canvas(w, h),
                    state.editor_zoom_fit,
                    state.editor_zoom_percent,
                    state.editor_pan,
                );
                self.target.PushAxisAlignedClip(
                    &to_d2d(screenshot_editor_canvas(w, h)),
                    D2D1_ANTIALIAS_MODE_ALIASED,
                );
                if document.background.is_none() {
                    self.checkerboard(
                        viewport,
                        screenshot_editor_canvas(w, h),
                        p.canvas_checker_a,
                        p.canvas_checker_b,
                    )?;
                }
                let result = self.bitmap_rect(image, viewport);
                self.target.PopAxisAlignedClip();
                result?;
            }

            if state.editor_tool == Tool::Select
                && let Some(document) = document
                && let Some(layer) = state.selected_layer.and_then(|id| {
                    document
                        .layers
                        .iter()
                        .find(|layer| layer.id == id && layer.visible && !layer.locked)
                })
                && let Some(corners) = layer.selection_corners()
            {
                let viewport = screenshot_editor_viewport(
                    (document.canvas_width, document.canvas_height),
                    screenshot_editor_canvas(w, h),
                    state.editor_zoom_fit,
                    state.editor_zoom_percent,
                    state.editor_pan,
                );
                let to_screen = |point: captures_windows_native::geometry::Point| Vector2 {
                    X: viewport.x
                        + (point.x - document.crop.x) / document.canvas_width as f32
                            * viewport.width,
                    Y: viewport.y
                        + (point.y - document.crop.y) / document.canvas_height as f32
                            * viewport.height,
                };
                let resize_handles = layer
                    .resize_handles()
                    .into_iter()
                    .map(|(_, point)| to_screen(point))
                    .collect::<Vec<_>>();
                let corners = corners.map(to_screen);
                let selection = self.brush(p.accent)?;
                for index in 0..4 {
                    self.target.DrawLine(
                        corners[index],
                        corners[(index + 1) % 4],
                        &selection,
                        1.5,
                        None,
                    );
                }
                let top = Vector2 {
                    X: (corners[0].X + corners[1].X) / 2.0,
                    Y: (corners[0].Y + corners[1].Y) / 2.0,
                };
                let center = Vector2 {
                    X: corners.iter().map(|point| point.X).sum::<f32>() / 4.0,
                    Y: corners.iter().map(|point| point.Y).sum::<f32>() / 4.0,
                };
                let mut direction = Vector2 {
                    X: top.X - center.X,
                    Y: top.Y - center.Y,
                };
                let mut length = direction.X.hypot(direction.Y);
                if length < 1.0 {
                    let axis = Vector2 {
                        X: corners[2].X - corners[0].X,
                        Y: corners[2].Y - corners[0].Y,
                    };
                    length = axis.X.hypot(axis.Y).max(1.0);
                    direction = Vector2 {
                        X: axis.Y,
                        Y: -axis.X,
                    };
                }
                let rotation = Vector2 {
                    X: top.X + direction.X / length * 28.0,
                    Y: top.Y + direction.Y / length * 28.0,
                };
                self.target.DrawLine(top, rotation, &selection, 1.5, None);
                let handle = self.brush(p.raised)?;
                for point in resize_handles.into_iter().chain([rotation]) {
                    let ellipse = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                        point,
                        radiusX: 5.0,
                        radiusY: 5.0,
                    };
                    self.target.FillEllipse(&ellipse, &handle);
                    self.target.DrawEllipse(&ellipse, &selection, 1.5, None);
                }
            }

            for (index, (tool, icon)) in [
                (Tool::Select, "select"),
                (Tool::Crop, "crop"),
                (Tool::Text, "text"),
                (Tool::Rectangle, "shapes"),
                (Tool::Arrow, "arrow"),
                (Tool::Pen, "pen"),
                (Tool::Eraser, "eraser"),
            ]
            .iter()
            .enumerate()
            {
                let rect = Rect {
                    x: 8.0,
                    y: 64.0 + index as f32 * 48.0,
                    width: 40.0,
                    height: 40.0,
                };
                let grouped = matches!(
                    state.editor_tool,
                    Tool::Line
                        | Tool::Rectangle
                        | Tool::Ellipse
                        | Tool::Triangle
                        | Tool::Diamond
                        | Tool::Star
                );
                if state.editor_tool == *tool || (*tool == Tool::Rectangle && grouped) {
                    self.rounded_panel(rect, p.accent, 7.0)?;
                }
                self.editor_icon(
                    icon,
                    rect.inset(10.0),
                    if state.editor_tool == *tool || (*tool == Tool::Rectangle && grouped) {
                        contrast_ink(p.accent)
                    } else {
                        p.text
                    },
                )?;
            }
            if state.editor_shapes_open {
                let flyout = Rect {
                    x: 60.0,
                    y: 204.0,
                    width: 292.0,
                    height: 146.0,
                };
                self.rounded_panel(flyout, p.raised, 10.0)?;
                self.text(
                    "Shapes",
                    Rect {
                        x: 72.0,
                        y: 212.0,
                        width: 268.0,
                        height: 24.0,
                    },
                    p.text,
                    &self.strong,
                );
                for (index, (name, label)) in [
                    ("line", "Line"),
                    ("rectangle", "Rectangle"),
                    ("ellipse", "Ellipse"),
                    ("triangle", "Triangle"),
                    ("diamond", "Diamond"),
                    ("star", "Star"),
                ]
                .iter()
                .enumerate()
                {
                    let rect = editor_shape_flyout_cell(index).expect("six shape cells");
                    self.rounded_panel(rect, p.field, 6.0)?;
                    self.editor_icon(
                        name,
                        Rect {
                            x: rect.x + 8.0,
                            y: rect.y + 10.0,
                            width: 20.0,
                            height: 20.0,
                        },
                        p.text,
                    )?;
                    self.text(
                        label,
                        Rect {
                            x: rect.x + 27.0,
                            y: rect.y + 9.0,
                            width: 59.0,
                            height: 22.0,
                        },
                        p.text,
                        &self.body,
                    );
                }
            }

            self.text(
                "Layers",
                Rect {
                    x: sidebar_x + 20.0,
                    y: 68.0,
                    width: 64.0,
                    height: 28.0,
                },
                p.text,
                &self.strong,
            );
            self.rounded_panel(
                Rect {
                    x: sidebar_x + 86.0,
                    y: 72.0,
                    width: 22.0,
                    height: 20.0,
                },
                p.field,
                10.0,
            )?;
            self.text(
                &document
                    .map_or(1, |value| value.layers.len() + 1)
                    .to_string(),
                Rect {
                    x: sidebar_x + 86.0,
                    y: 70.0,
                    width: 22.0,
                    height: 22.0,
                },
                p.muted,
                &self.body,
            );
            self.editor_icon(
                "plus",
                Rect {
                    x: sidebar_x + 276.0,
                    y: 75.0,
                    width: 14.0,
                    height: 14.0,
                },
                p.border,
            )?;
            self.divider(
                Vector2 {
                    X: sidebar_x,
                    Y: 100.0,
                },
                Vector2 { X: w, Y: 100.0 },
                p.border,
            )?;
            let mut layer_y = 104.0;
            if let Some(document) = document {
                for layer in document.layers.iter().rev().take(3) {
                    let selected = state.selected_layer == Some(layer.id);
                    self.panel(
                        Rect {
                            x: sidebar_x + 16.0,
                            y: layer_y,
                            width: 288.0,
                            height: 48.0,
                        },
                        if selected { p.field } else { p.raised },
                    );
                    self.editor_icon(
                        shape_icon(&layer.shape),
                        Rect {
                            x: sidebar_x + 28.0,
                            y: layer_y + 14.0,
                            width: 20.0,
                            height: 20.0,
                        },
                        p.text,
                    )?;
                    self.text(
                        &layer.name,
                        Rect {
                            x: sidebar_x + 58.0,
                            y: layer_y + 7.0,
                            width: 154.0,
                            height: 32.0,
                        },
                        p.text,
                        &self.body,
                    );
                    let eye = editor_layer_visibility_button(sidebar_x, layer_y);
                    self.editor_icon(
                        if layer.visible { "eye" } else { "eye-off" },
                        eye.inset(4.0),
                        p.muted,
                    )?;
                    let lock = editor_layer_lock_button(sidebar_x, layer_y);
                    self.editor_icon(
                        if layer.locked { "lock" } else { "unlock" },
                        lock.inset(5.0),
                        if layer.locked { p.accent } else { p.muted },
                    )?;
                    layer_y += 52.0;
                }
            }
            self.panel(
                Rect {
                    x: sidebar_x + 16.0,
                    y: layer_y,
                    width: 288.0,
                    height: 48.0,
                },
                p.field,
            );
            self.editor_icon(
                "image",
                Rect {
                    x: sidebar_x + 28.0,
                    y: layer_y + 14.0,
                    width: 20.0,
                    height: 20.0,
                },
                p.muted,
            )?;
            self.text(
                "Original screenshot\nLocked background",
                Rect {
                    x: sidebar_x + 58.0,
                    y: layer_y + 6.0,
                    width: 144.0,
                    height: 36.0,
                },
                p.muted,
                &self.body,
            );
            let source_eye = editor_layer_visibility_button(sidebar_x, layer_y);
            self.editor_icon(
                if document.is_some_and(|document| document.source_visible) {
                    "eye"
                } else {
                    "eye-off"
                },
                source_eye.inset(4.0),
                p.muted,
            )?;
            self.editor_icon(
                "lock",
                Rect {
                    x: sidebar_x + 268.0,
                    y: layer_y + 16.0,
                    width: 16.0,
                    height: 16.0,
                },
                p.muted,
            )?;
            let properties_y = (layer_y + 72.0).min(footer_y - 194.0);
            let selected_layer = state.selected_layer.and_then(|id| {
                document.and_then(|document| document.layers.iter().find(|layer| layer.id == id))
            });
            if selected_layer.is_some() || state.editor_tool != Tool::Select {
                self.divider(
                    Vector2 {
                        X: sidebar_x,
                        Y: properties_y - 12.0,
                    },
                    Vector2 {
                        X: w,
                        Y: properties_y - 12.0,
                    },
                    p.border,
                )?;
                if state.editor_tool == Tool::Eraser {
                    self.text(
                        "Eraser",
                        Rect {
                            x: sidebar_x + 20.0,
                            y: properties_y,
                            width: 272.0,
                            height: 24.0,
                        },
                        p.text,
                        &self.strong,
                    );
                    for (index, (mode, label)) in [
                        (RemoveBackgroundMode::Wand, "Wand"),
                        (RemoveBackgroundMode::Erase, "Erase"),
                        (RemoveBackgroundMode::Restore, "Restore"),
                    ]
                    .iter()
                    .enumerate()
                    {
                        self.button(
                            Rect {
                                x: sidebar_x + 20.0 + index as f32 * 92.0,
                                y: properties_y + 28.0,
                                width: 86.0,
                                height: 32.0,
                            },
                            if state.editor_remove_mode == *mode {
                                p.accent
                            } else {
                                p.field
                            },
                            if state.editor_remove_mode == *mode {
                                contrast_ink(p.accent)
                            } else {
                                p.text
                            },
                            label,
                        );
                    }
                    if state.editor_remove_mode == RemoveBackgroundMode::Wand {
                        self.property_stepper(
                            captures_windows_native::geometry::Point {
                                x: sidebar_x,
                                y: properties_y + 78.0,
                            },
                            "Tolerance",
                            &state.editor_wand_tolerance.to_string(),
                            "−8",
                            "+8",
                            p,
                        );
                        self.text(
                            "Contiguous only",
                            Rect {
                                x: sidebar_x + 20.0,
                                y: properties_y + 120.0,
                                width: 180.0,
                                height: 28.0,
                            },
                            p.muted,
                            &self.body,
                        );
                        self.toggle(
                            Rect {
                                x: sidebar_x + 254.0,
                                y: properties_y + 124.0,
                                width: 30.0,
                                height: 18.0,
                            },
                            p,
                            state.editor_wand_contiguous,
                            false,
                        )?;
                    } else {
                        self.property_stepper(
                            captures_windows_native::geometry::Point {
                                x: sidebar_x,
                                y: properties_y + 78.0,
                            },
                            "Size",
                            &format!("{} px", state.editor_remove_brush_size),
                            "−4",
                            "+4",
                            p,
                        );
                        self.property_stepper(
                            captures_windows_native::geometry::Point {
                                x: sidebar_x,
                                y: properties_y + 118.0,
                            },
                            "Softness",
                            &format!("{}%", state.editor_remove_softness),
                            "−10%",
                            "+10%",
                            p,
                        );
                    }
                    self.text(
                        "Choose an image, then click or paint on its pixels.",
                        Rect {
                            x: sidebar_x + 20.0,
                            y: properties_y + 158.0,
                            width: 272.0,
                            height: 34.0,
                        },
                        p.muted,
                        &self.body,
                    );
                } else {
                    let property_color =
                        selected_layer.map_or(state.editor_color, |layer| layer.color);
                    let property_stroke =
                        selected_layer.map_or(state.editor_stroke, |layer| layer.stroke);
                    let property_fill =
                        selected_layer.map_or(state.editor_fill, |layer| layer.fill);
                    let property_color_hex = format!(
                        "#{:02x}{:02x}{:02x}",
                        property_color[0], property_color[1], property_color[2]
                    );
                    self.text(
                        &selected_layer.map_or_else(
                            || tool_label(state.editor_tool).to_owned(),
                            |layer| blend_mode_label(layer.blend_mode).to_owned(),
                        ),
                        Rect {
                            x: sidebar_x + 20.0,
                            y: properties_y,
                            width: 90.0,
                            height: 24.0,
                        },
                        p.text,
                        &self.strong,
                    );
                    if selected_layer.is_some() {
                        for (index, label) in ["Front", "Back", "Copy", "Delete"].iter().enumerate()
                        {
                            self.button(
                                Rect {
                                    x: sidebar_x + 112.0 + index as f32 * 46.0,
                                    y: properties_y - 3.0,
                                    width: 44.0,
                                    height: 28.0,
                                },
                                p.field,
                                if *label == "Delete" { p.signal } else { p.text },
                                label,
                            );
                        }
                    }
                    if let Some((image_width, image_height, opacity, rotation)) = selected_layer
                        .and_then(|layer| match &layer.shape {
                            captures_windows_native::editor::Shape::Image {
                                width, height, ..
                            } => Some((*width, *height, layer.opacity, layer.rotation_degrees)),
                            _ => None,
                        })
                    {
                        self.property_value(
                            sidebar_x,
                            properties_y + 34.0,
                            "Blend mode",
                            blend_mode_label(selected_layer.expect("selected image").blend_mode),
                            p,
                        );
                        self.property_value(
                            sidebar_x,
                            properties_y + 64.0,
                            "Dimensions",
                            &format!("{image_width:.0} × {image_height:.0} px"),
                            p,
                        );
                        self.property_stepper(
                            captures_windows_native::geometry::Point {
                                x: sidebar_x,
                                y: properties_y + 94.0,
                            },
                            "Opacity",
                            &format!("{}%", (u16::from(opacity) * 100 + 127) / 255),
                            "−10%",
                            "+10%",
                            p,
                        );
                        self.property_stepper(
                            captures_windows_native::geometry::Point {
                                x: sidebar_x,
                                y: properties_y + 134.0,
                            },
                            "Rotation",
                            &format!("{rotation:.0}°"),
                            "−15°",
                            "+15°",
                            p,
                        );
                    } else {
                        self.text(
                            "Stroke color",
                            Rect {
                                x: sidebar_x + 20.0,
                                y: properties_y + 34.0,
                                width: 92.0,
                                height: 28.0,
                            },
                            p.muted,
                            &self.body,
                        );
                        self.button(
                            Rect {
                                x: sidebar_x + 116.0,
                                y: properties_y + 31.0,
                                width: 132.0,
                                height: 32.0,
                            },
                            p.field,
                            p.text,
                            if state.editor_editing_color {
                                &state.editor_color_hex
                            } else {
                                &property_color_hex
                            },
                        );
                        let swatch = self.brush(Color(
                            property_color[0],
                            property_color[1],
                            property_color[2],
                            property_color[3],
                        ))?;
                        self.target.FillEllipse(
                            &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                                point: Vector2 {
                                    X: sidebar_x + 276.0,
                                    Y: properties_y + 47.0,
                                },
                                radiusX: 13.0,
                                radiusY: 13.0,
                            },
                            &swatch,
                        );
                        self.text(
                            "Stroke width",
                            Rect {
                                x: sidebar_x + 20.0,
                                y: properties_y + 74.0,
                                width: 100.0,
                                height: 28.0,
                            },
                            p.muted,
                            &self.body,
                        );
                        self.button(
                            Rect {
                                x: sidebar_x + 184.0,
                                y: properties_y + 70.0,
                                width: 52.0,
                                height: 32.0,
                            },
                            p.field,
                            p.text,
                            "−",
                        );
                        self.text(
                            &format!("{property_stroke:.0} px"),
                            Rect {
                                x: sidebar_x + 120.0,
                                y: properties_y + 74.0,
                                width: 64.0,
                                height: 28.0,
                            },
                            p.text,
                            &self.body,
                        );
                        self.button(
                            Rect {
                                x: sidebar_x + 240.0,
                                y: properties_y + 70.0,
                                width: 52.0,
                                height: 32.0,
                            },
                            p.field,
                            p.text,
                            "+",
                        );
                        self.text(
                            if selected_layer.is_none_or(|layer| layer.supports_fill()) {
                                "Fill shape"
                            } else {
                                "Fill unavailable"
                            },
                            Rect {
                                x: sidebar_x + 20.0,
                                y: properties_y + 114.0,
                                width: 150.0,
                                height: 28.0,
                            },
                            p.muted,
                            &self.body,
                        );
                        if selected_layer.is_none_or(|layer| layer.supports_fill()) {
                            self.toggle(
                                Rect {
                                    x: sidebar_x + 248.0,
                                    y: properties_y + 118.0,
                                    width: 36.0,
                                    height: 20.0,
                                },
                                p,
                                property_fill.is_some(),
                                false,
                            )?;
                        }
                        self.text(
                            "Rotation",
                            Rect {
                                x: sidebar_x + 20.0,
                                y: properties_y + 154.0,
                                width: 90.0,
                                height: 28.0,
                            },
                            p.muted,
                            &self.body,
                        );
                        if let Some(layer) = selected_layer {
                            self.text(
                                &format!("{:.0}°", layer.rotation_degrees),
                                Rect {
                                    x: sidebar_x + 120.0,
                                    y: properties_y + 154.0,
                                    width: 64.0,
                                    height: 28.0,
                                },
                                p.text,
                                &self.body,
                            );
                            self.button(
                                Rect {
                                    x: sidebar_x + 184.0,
                                    y: properties_y + 150.0,
                                    width: 52.0,
                                    height: 32.0,
                                },
                                p.field,
                                p.text,
                                "−15°",
                            );
                            self.button(
                                Rect {
                                    x: sidebar_x + 240.0,
                                    y: properties_y + 150.0,
                                    width: 52.0,
                                    height: 32.0,
                                },
                                p.field,
                                p.text,
                                "+15°",
                            );
                            self.property_stepper(
                                captures_windows_native::geometry::Point {
                                    x: sidebar_x,
                                    y: properties_y + 194.0,
                                },
                                "Opacity",
                                &format!("{}%", (u16::from(layer.opacity) * 100 + 127) / 255),
                                "−10%",
                                "+10%",
                                p,
                            );
                        } else {
                            self.text(
                                "Select a layer to rotate",
                                Rect {
                                    x: sidebar_x + 120.0,
                                    y: properties_y + 154.0,
                                    width: 172.0,
                                    height: 28.0,
                                },
                                p.muted,
                                &self.body,
                            );
                        }
                    }
                }
            }

            if state.editor_export_settings_open {
                self.panel(
                    Rect {
                        x: 16.0,
                        y: footer_y - 132.0,
                        width: w - 32.0,
                        height: 120.0,
                    },
                    p.field,
                );
                let output = document.map_or((0, 0), |document| {
                    state.editor_export_size.dimensions(
                        document,
                        (
                            state.editor_custom_export_width,
                            state.editor_custom_export_height,
                        ),
                    )
                });
                self.text(
                    &format!(
                        "Output size     {}  ▾     {} × {}",
                        state.editor_export_size.label(),
                        output.0,
                        output.1
                    ),
                    Rect {
                        x: 30.0,
                        y: footer_y - 122.0,
                        width: 390.0,
                        height: 28.0,
                    },
                    p.text,
                    &self.body,
                );
                self.text(
                    &format!(
                        "Save quality    {}  ▾{}",
                        state.editor_quality_mode.label(),
                        if state.editor_quality_mode
                            == captures_windows_native::state::EditorQualityMode::Compress
                        {
                            format!("     Quality {}", state.editor_quality)
                        } else if state.editor_quality_mode
                            == captures_windows_native::state::EditorQualityMode::Maximum
                        {
                            format!("     Maximum {} KB", state.editor_maximum_kilobytes)
                        } else {
                            String::new()
                        }
                    ),
                    Rect {
                        x: 30.0,
                        y: footer_y - 86.0,
                        width: 560.0,
                        height: 28.0,
                    },
                    p.text,
                    &self.body,
                );
                self.text(
                    &format!(
                        "Est. size       {}",
                        if state.editor_estimate_pending {
                            "Estimating…".to_owned()
                        } else {
                            state
                                .editor_estimated_bytes
                                .map_or_else(|| "—".to_owned(), format_bytes)
                        }
                    ),
                    Rect {
                        x: 30.0,
                        y: footer_y - 50.0,
                        width: 390.0,
                        height: 28.0,
                    },
                    p.muted,
                    &self.body,
                );
            }
            self.button(
                Rect {
                    x: 16.0,
                    y: footer_y + 18.0,
                    width: 184.0,
                    height: 56.0,
                },
                p.field,
                p.text,
                &format!(
                    "Export settings\n{} · {} · {}",
                    state.editor_format.to_uppercase(),
                    state.editor_export_size.label(),
                    state.editor_quality_mode.label()
                ),
            );
            self.editor_icon(
                "chevron-down",
                Rect {
                    x: 176.0,
                    y: footer_y + 39.0,
                    width: 12.0,
                    height: 12.0,
                },
                p.muted,
            )?;
            self.text(
                "Filename",
                Rect {
                    x: 216.0,
                    y: footer_y + 8.0,
                    width: 270.0,
                    height: 18.0,
                },
                p.muted,
                &self.body,
            );
            self.text(
                "Saving to Captures folder",
                Rect {
                    x: 500.0,
                    y: footer_y + 8.0,
                    width: 226.0,
                    height: 18.0,
                },
                p.muted,
                &self.body,
            );
            self.button(
                Rect {
                    x: 216.0,
                    y: footer_y + 30.0,
                    width: 198.0,
                    height: 42.0,
                },
                p.field,
                p.text,
                &state.editor_filename,
            );
            self.button(
                Rect {
                    x: 418.0,
                    y: footer_y + 30.0,
                    width: 68.0,
                    height: 42.0,
                },
                p.field,
                p.text,
                &format!(".{}", state.editor_format),
            );
            self.button(
                Rect {
                    x: w - 492.0,
                    y: footer_y + 30.0,
                    width: 122.0,
                    height: 42.0,
                },
                p.field,
                p.text,
                "     Copy image",
            );
            self.editor_icon(
                "copy",
                Rect {
                    x: w - 478.0,
                    y: footer_y + 42.0,
                    width: 16.0,
                    height: 16.0,
                },
                p.text,
            )?;
            let can_replace = can_replace_editor_source(
                state.editor_source.as_deref(),
                &state.editor_format,
                &state.editor_filename,
            );
            self.toggle(
                Rect {
                    x: w - 348.0,
                    y: footer_y + 42.0,
                    width: 28.0,
                    height: 16.0,
                },
                p,
                state.editor_save_as_new,
                !can_replace,
            )?;
            self.text(
                "Save as new file",
                Rect {
                    x: w - 314.0,
                    y: footer_y + 34.0,
                    width: 122.0,
                    height: 32.0,
                },
                p.text,
                &self.body,
            );
            self.button(
                Rect {
                    x: w - 168.0,
                    y: footer_y + 26.0,
                    width: 148.0,
                    height: 46.0,
                },
                p.accent,
                contrast_ink(p.accent),
                "     Save",
            );
            self.editor_icon(
                "save",
                Rect {
                    x: w - 142.0,
                    y: footer_y + 41.0,
                    width: 17.0,
                    height: 17.0,
                },
                contrast_ink(p.accent),
            )?;
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
                    ["select", "window", "display"][i],
                )?;
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
            for (index, (label, icon, enabled)) in [
                ("Cursor", "select", settings.recording.show_cursor),
                ("Clicks", "clicks", settings.recording.highlight_clicks),
                ("Keys", "keys", settings.recording.show_keystrokes),
                ("Audio", "audio", settings.recording.capture_system_audio),
            ]
            .iter()
            .enumerate()
            {
                let x = 4.0 + index as f32 * 104.0;
                self.rounded_panel(
                    Rect {
                        x,
                        y: 326.0,
                        width: 98.0,
                        height: 44.0,
                    },
                    p.field,
                    7.0,
                )?;
                self.editor_icon(
                    icon,
                    Rect {
                        x: x + 9.0,
                        y: 334.0,
                        width: 14.0,
                        height: 14.0,
                    },
                    p.muted,
                )?;
                self.text(
                    label,
                    Rect {
                        x: x + 26.0,
                        y: 330.0,
                        width: 62.0,
                        height: 20.0,
                    },
                    p.text,
                    &self.body,
                );
                self.toggle(
                    Rect {
                        x: x + 10.0,
                        y: 350.0,
                        width: 30.0,
                        height: 18.0,
                    },
                    p,
                    *enabled,
                    false,
                )?;
                self.text(
                    if *enabled { "On" } else { "Off" },
                    Rect {
                        x: x + 44.0,
                        y: 347.0,
                        width: 44.0,
                        height: 22.0,
                    },
                    p.muted,
                    &self.body,
                );
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
                "Edit recording",
                Rect {
                    x: 24.0,
                    y: 18.0,
                    width: 220.0,
                    height: 32.0,
                },
                p.text,
                &self.title,
            );
            self.rounded_panel(
                Rect {
                    x: 24.0,
                    y: 66.0,
                    width: w - 280.0,
                    height: h - 286.0,
                },
                p.raised,
                10.0,
            )?;
            self.text(
                "Preview",
                Rect {
                    x: 42.0,
                    y: 74.0,
                    width: 80.0,
                    height: 32.0,
                },
                p.muted,
                &self.strong,
            );
            self.rounded_panel(
                Rect {
                    x: w - 414.0,
                    y: 75.0,
                    width: 142.0,
                    height: 28.0,
                },
                p.canvas,
                7.0,
            )?;
            self.text(
                "Fit · unavailable",
                Rect {
                    x: w - 408.0,
                    y: 76.0,
                    width: 130.0,
                    height: 24.0,
                },
                p.muted,
                &self.body,
            );
            self.panel(
                Rect {
                    x: 32.0,
                    y: 108.0,
                    width: w - 296.0,
                    height: h - 336.0,
                },
                Color(11, 11, 14, 255),
            );
            if let Some(preview) = &state.recording_preview {
                self.bitmap_contain(
                    preview,
                    Rect {
                        x: 40.0,
                        y: 116.0,
                        width: w - 312.0,
                        height: h - 352.0,
                    },
                )?;
            }
            self.rounded_panel(
                Rect {
                    x: w - 236.0,
                    y: 66.0,
                    width: 212.0,
                    height: h - 286.0,
                },
                p.raised,
                10.0,
            )?;
            self.text(
                "Crop & size",
                Rect {
                    x: w - 216.0,
                    y: 80.0,
                    width: 172.0,
                    height: 24.0,
                },
                p.text,
                &self.strong,
            );
            self.text(
                "Original resolution\nCrop controls unavailable",
                Rect {
                    x: w - 216.0,
                    y: 106.0,
                    width: 172.0,
                    height: 44.0,
                },
                p.muted,
                &self.body,
            );
            self.divider(
                Vector2 {
                    X: w - 216.0,
                    Y: 160.0,
                },
                Vector2 {
                    X: w - 44.0,
                    Y: 160.0,
                },
                p.border,
            )?;
            self.text(
                "Audio",
                Rect {
                    x: w - 216.0,
                    y: 172.0,
                    width: 172.0,
                    height: 24.0,
                },
                p.text,
                &self.strong,
            );
            self.text(
                if editor.is_some_and(|editor| editor.has_audio) {
                    "Preserved on export\nPreview audio unavailable"
                } else {
                    "No recorded audio"
                },
                Rect {
                    x: w - 216.0,
                    y: 198.0,
                    width: 172.0,
                    height: 42.0,
                },
                p.muted,
                &self.body,
            );
            self.divider(
                Vector2 {
                    X: w - 216.0,
                    Y: 248.0,
                },
                Vector2 {
                    X: w - 44.0,
                    Y: 248.0,
                },
                p.border,
            )?;
            self.text(
                "Save quality",
                Rect {
                    x: w - 216.0,
                    y: 258.0,
                    width: 172.0,
                    height: 24.0,
                },
                p.text,
                &self.strong,
            );
            self.rounded_panel(
                Rect {
                    x: w - 216.0,
                    y: 286.0,
                    width: 172.0,
                    height: 34.0,
                },
                p.field,
                7.0,
            )?;
            self.text(
                match editor.map(|editor| editor.quality) {
                    Some(captures_media::QualityPreset::Preserve) | None => "Preserve quality  ▾",
                    Some(captures_media::QualityPreset::Highest) => "Highest quality   ▾",
                    Some(captures_media::QualityPreset::High) => "High quality      ▾",
                    Some(captures_media::QualityPreset::Standard) => "Standard quality  ▾",
                    Some(captures_media::QualityPreset::Small) => "Small file        ▾",
                    Some(captures_media::QualityPreset::Tiny) => "Tiny file         ▾",
                },
                Rect {
                    x: w - 210.0,
                    y: 290.0,
                    width: 160.0,
                    height: 26.0,
                },
                p.text,
                &self.body,
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
            let timeline_y = h - 196.0;
            self.rounded_panel(
                Rect {
                    x: 24.0,
                    y: timeline_y,
                    width: w - 48.0,
                    height: 96.0,
                },
                p.raised,
                10.0,
            )?;
            self.text(
                &editor.map_or_else(
                    || "00:00 – End".to_owned(),
                    |editor| {
                        format!(
                            "{} – {}                         {} selected",
                            format_time(editor.trim_start_ms),
                            format_time(editor.trim_end_ms),
                            format_time(editor.trim_end_ms.saturating_sub(editor.trim_start_ms))
                        )
                    },
                ),
                Rect {
                    x: 44.0,
                    y: timeline_y + 8.0,
                    width: w - 88.0,
                    height: 22.0,
                },
                p.muted,
                &self.body,
            );
            let play_rect = Rect {
                x: 40.0,
                y: timeline_y + 42.0,
                width: 32.0,
                height: 32.0,
            };
            self.rounded_panel(play_rect, p.field, 16.0)?;
            self.editor_icon(
                if editor.is_some_and(|editor| editor.playing) {
                    "pause"
                } else {
                    "play"
                },
                play_rect.inset(10.0),
                p.text,
            )?;
            let track = recording_editor_timeline_track(w, h);
            self.rounded_panel(track, p.field, 6.0)?;
            if let Some(preview) = &state.recording_preview {
                for index in 0..8 {
                    self.bitmap_contain(
                        preview,
                        Rect {
                            x: track.x + 4.0 + index as f32 * (track.width - 8.0) / 8.0,
                            y: track.y + 4.0,
                            width: (track.width - 8.0) / 8.0,
                            height: track.height - 8.0,
                        },
                    )?;
                }
            }
            if let Some(editor) = editor {
                let duration = editor.duration_ms.max(1) as f32;
                let start_x = track.x + editor.trim_start_ms as f32 / duration * track.width;
                let end_x = track.x + editor.trim_end_ms as f32 / duration * track.width;
                let playhead_x = track.x + editor.position_ms as f32 / duration * track.width;
                let excluded = self.brush(Color(p.canvas.0, p.canvas.1, p.canvas.2, 190))?;
                self.target.FillRectangle(
                    &D2D_RECT_F {
                        left: track.x,
                        top: track.y,
                        right: start_x,
                        bottom: track.y + track.height,
                    },
                    &excluded,
                );
                self.target.FillRectangle(
                    &D2D_RECT_F {
                        left: end_x,
                        top: track.y,
                        right: track.x + track.width,
                        bottom: track.y + track.height,
                    },
                    &excluded,
                );
                let trim = self.brush(p.accent)?;
                for x in [start_x, end_x] {
                    self.target.FillRoundedRectangle(
                        &windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: x - 5.0,
                                top: track.y - 3.0,
                                right: x + 5.0,
                                bottom: track.y + track.height + 3.0,
                            },
                            radiusX: 3.0,
                            radiusY: 3.0,
                        },
                        &trim,
                    );
                }
                self.divider(
                    Vector2 {
                        X: playhead_x,
                        Y: track.y - 2.0,
                    },
                    Vector2 {
                        X: playhead_x,
                        Y: track.y + track.height + 2.0,
                    },
                    p.signal,
                )?;
            }
            let footer_y = h - 88.0;
            self.panel(
                Rect {
                    x: 0.0,
                    y: footer_y,
                    width: w,
                    height: 88.0,
                },
                p.raised,
            );
            self.divider(
                Vector2 {
                    X: 0.0,
                    Y: footer_y,
                },
                Vector2 { X: w, Y: footer_y },
                p.border,
            )?;
            self.text(
                "Filename",
                Rect {
                    x: 24.0,
                    y: footer_y + 8.0,
                    width: 250.0,
                    height: 18.0,
                },
                p.muted,
                &self.body,
            );
            let filename = editor
                .and_then(|editor| editor.source.file_stem())
                .and_then(|value| value.to_str())
                .unwrap_or("Recording");
            self.button(
                Rect {
                    x: 24.0,
                    y: footer_y + 30.0,
                    width: 280.0,
                    height: 40.0,
                },
                p.field,
                p.text,
                &format!("{filename}                                      .mp4"),
            );
            self.toggle(
                Rect {
                    x: w - 374.0,
                    y: footer_y + 41.0,
                    width: 30.0,
                    height: 18.0,
                },
                p,
                editor.is_none_or(|editor| editor.save_as_new),
                false,
            )?;
            self.text(
                "Save as new file",
                Rect {
                    x: w - 338.0,
                    y: footer_y + 36.0,
                    width: 134.0,
                    height: 28.0,
                },
                p.text,
                &self.body,
            );
            self.button(
                Rect {
                    x: w - 178.0,
                    y: footer_y + 27.0,
                    width: 154.0,
                    height: 44.0,
                },
                p.accent,
                contrast_ink(p.accent),
                "     Export",
            );
            self.editor_icon(
                "save",
                Rect {
                    x: w - 150.0,
                    y: footer_y + 41.0,
                    width: 16.0,
                    height: 16.0,
                },
                contrast_ink(p.accent),
            )?;
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
                for (action, (icon, label)) in [
                    ("pen", "Edit"),
                    ("copy", "Copy"),
                    ("select", "Drag"),
                    ("trash", "Delete"),
                ]
                .iter()
                .enumerate()
                {
                    let action_x = 4.0 + action as f32 * (w - 8.0) / 4.0;
                    self.editor_icon(
                        icon,
                        Rect {
                            x: action_x + 5.0,
                            y: y + 173.0,
                            width: 14.0,
                            height: 14.0,
                        },
                        if *label == "Delete" {
                            p.signal
                        } else {
                            Color(246, 246, 248, 220)
                        },
                    )?;
                    self.text(
                        label,
                        Rect {
                            x: action_x + 20.0,
                            y: y + 169.0,
                            width: (w - 8.0) / 4.0 - 22.0,
                            height: 24.0,
                        },
                        if *label == "Delete" {
                            p.signal
                        } else {
                            Color(246, 246, 248, 220)
                        },
                        &self.strong,
                    );
                }
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
                        &format!("   {label}"),
                    );
                    self.editor_icon(
                        ["pen", "undo", "trash"][button],
                        Rect {
                            x: x + button as f32 * 57.0 + 5.0,
                            y: 225.0,
                            width: 12.0,
                            height: 12.0,
                        },
                        if !enabled {
                            p.muted
                        } else if *label == "Delete" {
                            p.signal
                        } else {
                            p.text
                        },
                    )?;
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
    unsafe fn preferences(
        &self,
        state: &AppState,
        settings: &Settings,
        p: Palette,
        w: f32,
        h: f32,
    ) -> Result<()> {
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
            for (index, page) in PreferencesPage::ALL.into_iter().enumerate() {
                let y = 72.0 + index as f32 * 44.0;
                if page == state.preferences_page {
                    self.rounded_panel(
                        Rect {
                            x: 12.0,
                            y,
                            width: 160.0,
                            height: 38.0,
                        },
                        p.field,
                        7.0,
                    )?;
                }
                self.text(
                    page.label(),
                    Rect {
                        x: 22.0,
                        y: y + 4.0,
                        width: 140.0,
                        height: 28.0,
                    },
                    if page == state.preferences_page {
                        p.text
                    } else {
                        p.muted
                    },
                    if page == state.preferences_page {
                        &self.strong
                    } else {
                        &self.body
                    },
                );
            }
            self.button(
                Rect {
                    x: 12.0,
                    y: h - 58.0,
                    width: 160.0,
                    height: 38.0,
                },
                p.field,
                p.text,
                "Send feedback",
            );
            self.text(
                state.preferences_page.label(),
                Rect {
                    x: 216.0,
                    y: 22.0,
                    width: w - 244.0,
                    height: 32.0,
                },
                p.text,
                &self.title,
            );
            let rows: Vec<(String, Option<bool>, String, String)> = match state.preferences_page {
                PreferencesPage::General => vec![
                    (
                        "Start Captures at login".into(),
                        Some(settings.launch_at_login),
                        "".into(),
                        "Open Captures when you sign in.".into(),
                    ),
                    (
                        "Copy captures automatically".into(),
                        Some(settings.auto_copy_to_clipboard),
                        "".into(),
                        "Copy new captures without replacing controls.".into(),
                    ),
                    (
                        "Show mini previews".into(),
                        Some(settings.show_mini_previews),
                        "".into(),
                        "Keep quick actions near the screen edge.".into(),
                    ),
                    (
                        "Freeze screen while selecting".into(),
                        Some(settings.freeze_screen),
                        "".into(),
                        "Hold menus, hover states, and motion still.".into(),
                    ),
                ],
                PreferencesPage::Capture => vec![
                    (
                        "Countdown".into(),
                        None,
                        format!("{} seconds", settings.screenshot_countdown_seconds),
                        "Delay before taking a screenshot.".into(),
                    ),
                    (
                        "Show cursor".into(),
                        Some(settings.show_cursor_in_screenshots),
                        "".into(),
                        "Include the pointer in screenshots.".into(),
                    ),
                    (
                        "Freeze screen".into(),
                        Some(settings.freeze_screen),
                        "".into(),
                        "Keep the captured selection visually stable.".into(),
                    ),
                    (
                        "Screenshot format".into(),
                        None,
                        settings.screenshot_format.to_uppercase(),
                        "Format used for new screenshots.".into(),
                    ),
                ],
                PreferencesPage::Recording => vec![
                    (
                        "Format".into(),
                        None,
                        settings.recording.video_format.to_uppercase(),
                        "Video or animated image output.".into(),
                    ),
                    (
                        "Frame rate".into(),
                        None,
                        format!("{} FPS", settings.recording.video_fps),
                        "Video capture frame rate.".into(),
                    ),
                    (
                        "Countdown".into(),
                        None,
                        format!("{} seconds", settings.recording.countdown_seconds),
                        "Delay before recording starts.".into(),
                    ),
                    (
                        "Show cursor".into(),
                        Some(settings.recording.show_cursor),
                        "".into(),
                        "Include the pointer in recordings.".into(),
                    ),
                    (
                        "System audio".into(),
                        Some(settings.recording.capture_system_audio),
                        "".into(),
                        "Capture desktop audio when supported.".into(),
                    ),
                    (
                        "Open editor after recording".into(),
                        Some(settings.recording.open_editor_after_recording),
                        "".into(),
                        "Review trim and export before saving.".into(),
                    ),
                ],
                PreferencesPage::Shortcuts => vec![
                    (
                        "Capture region".into(),
                        None,
                        "Ctrl + Shift + 4".into(),
                        "Global shortcut; registration conflicts fail closed.".into(),
                    ),
                    (
                        "Capture display".into(),
                        None,
                        "Ctrl + Shift + 3".into(),
                        "Global shortcut; registration conflicts fail closed.".into(),
                    ),
                ],
                PreferencesPage::Appearance => vec![
                    (
                        "Appearance".into(),
                        None,
                        settings.appearance.clone(),
                        "Follow Windows or choose light/dark.".into(),
                    ),
                    (
                        "Color theme".into(),
                        None,
                        settings.theme.clone(),
                        "Accent for capture actions and selection.".into(),
                    ),
                    (
                        "Include previews in captures".into(),
                        Some(settings.include_mini_previews_in_captures),
                        "".into(),
                        "Allow mini previews to appear in captures.".into(),
                    ),
                    (
                        "Include recording controls".into(),
                        Some(settings.include_recording_controls_in_captures),
                        "".into(),
                        "Allow the HUD to appear in captures.".into(),
                    ),
                ],
            };
            for (i, (label, toggle, value, description)) in rows.iter().enumerate() {
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
                self.text(
                    description,
                    Rect {
                        x: 216.0,
                        y: y + 24.0,
                        width: w - 390.0,
                        height: 22.0,
                    },
                    p.muted,
                    &self.body,
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
                        false,
                    )?;
                } else {
                    self.rounded_panel(
                        Rect {
                            x: w - 152.0,
                            y: y - 4.0,
                            width: 120.0,
                            height: 32.0,
                        },
                        p.field,
                        7.0,
                    )?;
                    self.text(
                        value,
                        Rect {
                            x: w - 146.0,
                            y,
                            width: 108.0,
                            height: 24.0,
                        },
                        p.text,
                        &self.body,
                    );
                }
                if i < rows.len() - 1 {
                    self.divider(
                        Vector2 {
                            X: 216.0,
                            Y: y + 54.0,
                        },
                        Vector2 {
                            X: w - 32.0,
                            Y: y + 54.0,
                        },
                        p.border,
                    )?;
                }
            }
            Ok(())
        }
    }
    unsafe fn feedback(&self, state: &AppState, p: Palette, w: f32, h: f32) -> Result<()> {
        unsafe {
            self.text(
                "Send feedback",
                Rect {
                    x: 32.0,
                    y: 24.0,
                    width: w - 64.0,
                    height: 34.0,
                },
                p.text,
                &self.title,
            );
            self.text(
                "Sent only when you press Send. Captures, files, logs, and diagnostics are never attached.",
                Rect { x: 32.0, y: 58.0, width: w - 64.0, height: 28.0 },
                p.muted,
                &self.body,
            );
            self.text(
                "Message",
                Rect {
                    x: 32.0,
                    y: 92.0,
                    width: 100.0,
                    height: 24.0,
                },
                p.text,
                &self.strong,
            );
            self.panel(
                Rect {
                    x: 32.0,
                    y: 118.0,
                    width: w - 64.0,
                    height: 132.0,
                },
                p.field,
            );
            self.text(
                if state.feedback_message.is_empty() {
                    "Describe the issue or idea…"
                } else {
                    &state.feedback_message
                },
                Rect {
                    x: 44.0,
                    y: 128.0,
                    width: w - 88.0,
                    height: 110.0,
                },
                if state.feedback_message.is_empty() {
                    p.muted
                } else {
                    p.text
                },
                &self.body,
            );
            self.text(
                "Contact (optional)",
                Rect {
                    x: 32.0,
                    y: 266.0,
                    width: 160.0,
                    height: 24.0,
                },
                p.text,
                &self.strong,
            );
            self.panel(
                Rect {
                    x: 32.0,
                    y: 292.0,
                    width: w - 64.0,
                    height: 38.0,
                },
                p.field,
            );
            self.text(
                if state.feedback_contact.is_empty() {
                    "Email or handle"
                } else {
                    &state.feedback_contact
                },
                Rect {
                    x: 44.0,
                    y: 299.0,
                    width: w - 88.0,
                    height: 24.0,
                },
                if state.feedback_contact.is_empty() {
                    p.muted
                } else {
                    p.text
                },
                &self.body,
            );
            for (index, category) in ["bug", "idea", "other"].iter().enumerate() {
                self.button(
                    Rect {
                        x: 32.0 + index as f32 * 112.0,
                        y: 348.0,
                        width: 104.0,
                        height: 36.0,
                    },
                    if state.feedback_category == *category {
                        p.accent
                    } else {
                        p.field
                    },
                    if state.feedback_category == *category {
                        Color(255, 255, 255, 255)
                    } else {
                        p.text
                    },
                    &category.to_ascii_uppercase(),
                );
            }
            self.button(
                Rect {
                    x: 32.0,
                    y: h - 58.0,
                    width: 120.0,
                    height: 38.0,
                },
                p.field,
                p.text,
                "Back",
            );
            self.button(
                Rect {
                    x: w - 160.0,
                    y: h - 58.0,
                    width: 128.0,
                    height: 38.0,
                },
                if state.feedback_submitting {
                    p.field
                } else {
                    p.accent
                },
                if state.feedback_submitting {
                    p.muted
                } else {
                    Color(255, 255, 255, 255)
                },
                if state.feedback_submitting {
                    "Sending…"
                } else {
                    "Send"
                },
            );
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

    unsafe fn card(
        &self,
        rect: Rect,
        p: Palette,
        title: &str,
        description: &str,
        icon: &str,
    ) -> Result<()> {
        unsafe {
            self.panel(rect, p.raised);
            self.rounded_panel(
                Rect {
                    x: rect.x + 12.0,
                    y: rect.y + 12.0,
                    width: 40.0,
                    height: 40.0,
                },
                Color(p.accent.0, p.accent.1, p.accent.2, 42),
                8.0,
            )?;
            self.editor_icon(
                icon,
                Rect {
                    x: rect.x + 23.0,
                    y: rect.y + 23.0,
                    width: 18.0,
                    height: 18.0,
                },
                p.accent,
            )?;
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
            );
            Ok(())
        }
    }
    unsafe fn panel(&self, rect: Rect, fill: Color) {
        unsafe {
            let brush = self.brush(fill).expect("brush");
            self.target.FillRectangle(&to_d2d(rect), &brush)
        }
    }
    unsafe fn checkerboard(
        &self,
        viewport: Rect,
        clip: Rect,
        checker_a: Color,
        checker_b: Color,
    ) -> Result<()> {
        unsafe {
            let left = viewport.x.max(clip.x);
            let top = viewport.y.max(clip.y);
            let right = (viewport.x + viewport.width).min(clip.x + clip.width);
            let bottom = (viewport.y + viewport.height).min(clip.y + clip.height);
            if right <= left || bottom <= top {
                return Ok(());
            }
            let visible = Rect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            };
            let background = self.brush(checker_b)?;
            self.target.FillRectangle(&to_d2d(visible), &background);

            const CHECK_SIZE: f32 = 8.0;
            let first_column = ((left - viewport.x) / CHECK_SIZE).floor() as i32;
            let last_column = ((right - viewport.x) / CHECK_SIZE).ceil() as i32;
            let first_row = ((top - viewport.y) / CHECK_SIZE).floor() as i32;
            let last_row = ((bottom - viewport.y) / CHECK_SIZE).ceil() as i32;
            let foreground = self.brush(checker_a)?;
            for row in first_row..last_row {
                for column in first_column..last_column {
                    if (row + column).rem_euclid(2) != 0 {
                        continue;
                    }
                    let raw_left = viewport.x + column as f32 * CHECK_SIZE;
                    let raw_top = viewport.y + row as f32 * CHECK_SIZE;
                    let square_left = raw_left.max(left);
                    let square_top = raw_top.max(top);
                    let square_right = (raw_left + CHECK_SIZE).min(right);
                    let square_bottom = (raw_top + CHECK_SIZE).min(bottom);
                    self.target.FillRectangle(
                        &D2D_RECT_F {
                            left: square_left,
                            top: square_top,
                            right: square_right,
                            bottom: square_bottom,
                        },
                        &foreground,
                    );
                }
            }
            Ok(())
        }
    }
    unsafe fn rounded_panel(&self, rect: Rect, fill: Color, radius: f32) -> Result<()> {
        unsafe {
            let brush = self.brush(fill)?;
            self.target.FillRoundedRectangle(
                &windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
                    rect: to_d2d(rect),
                    radiusX: radius,
                    radiusY: radius,
                },
                &brush,
            );
            Ok(())
        }
    }
    unsafe fn divider(&self, from: Vector2, to: Vector2, color: Color) -> Result<()> {
        unsafe {
            let brush = self.brush(color)?;
            self.target.DrawLine(from, to, &brush, 1.0, None);
            Ok(())
        }
    }
    unsafe fn button(&self, rect: Rect, fill: Color, ink: Color, label: &str) {
        unsafe {
            self.panel(rect, fill);
            self.text(label, rect.inset(4.0), ink, &self.strong)
        }
    }
    unsafe fn property_value(&self, sidebar_x: f32, y: f32, label: &str, value: &str, p: Palette) {
        unsafe {
            self.text(
                label,
                Rect {
                    x: sidebar_x + 20.0,
                    y,
                    width: 96.0,
                    height: 28.0,
                },
                p.muted,
                &self.body,
            );
            self.text(
                value,
                Rect {
                    x: sidebar_x + 120.0,
                    y,
                    width: 172.0,
                    height: 28.0,
                },
                p.text,
                &self.body,
            );
        }
    }
    unsafe fn property_stepper(
        &self,
        origin: captures_windows_native::geometry::Point,
        label: &str,
        value: &str,
        decrement: &str,
        increment: &str,
        p: Palette,
    ) {
        unsafe {
            self.text(
                label,
                Rect {
                    x: origin.x + 20.0,
                    y: origin.y,
                    width: 96.0,
                    height: 28.0,
                },
                p.muted,
                &self.body,
            );
            self.text(
                value,
                Rect {
                    x: origin.x + 120.0,
                    y: origin.y,
                    width: 64.0,
                    height: 28.0,
                },
                p.text,
                &self.body,
            );
            self.button(
                Rect {
                    x: origin.x + 184.0,
                    y: origin.y - 4.0,
                    width: 52.0,
                    height: 32.0,
                },
                p.field,
                p.text,
                decrement,
            );
            self.button(
                Rect {
                    x: origin.x + 240.0,
                    y: origin.y - 4.0,
                    width: 52.0,
                    height: 32.0,
                },
                p.field,
                p.text,
                increment,
            );
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

    unsafe fn toggle(&self, rect: Rect, p: Palette, checked: bool, disabled: bool) -> Result<()> {
        unsafe {
            let rect = Rect {
                x: rect.x,
                y: rect.y,
                width: 30.0,
                height: 18.0,
            };
            let track_color = if disabled {
                Color(p.border.0, p.border.1, p.border.2, 90)
            } else if checked {
                p.accent
            } else {
                p.field
            };
            let track = self.brush(track_color)?;
            self.target.FillRoundedRectangle(
                &windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
                    rect: to_d2d(rect),
                    radiusX: 9.0,
                    radiusY: 9.0,
                },
                &track,
            );
            let outline = self.brush(if checked { track_color } else { p.border })?;
            self.target.DrawRoundedRectangle(
                &windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
                    rect: to_d2d(rect),
                    radiusX: 9.0,
                    radiusY: 9.0,
                },
                &outline,
                1.0,
                None,
            );
            let knob = self.brush(if disabled {
                Color(p.muted.0, p.muted.1, p.muted.2, 100)
            } else if checked {
                contrast_ink(p.accent)
            } else {
                p.muted
            })?;
            self.target.FillEllipse(
                &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                    point: Vector2 {
                        X: if checked { rect.x + 21.0 } else { rect.x + 9.0 },
                        Y: rect.y + 9.0,
                    },
                    radiusX: 6.0,
                    radiusY: 6.0,
                },
                &knob,
            );
            Ok(())
        }
    }

    unsafe fn editor_icon(&self, name: &str, rect: Rect, ink: Color) -> Result<()> {
        unsafe {
            let brush = self.brush(ink)?;
            let left = rect.x;
            let top = rect.y;
            let right = rect.x + rect.width;
            let bottom = rect.y + rect.height;
            let cx = rect.x + rect.width / 2.0;
            let cy = rect.y + rect.height / 2.0;
            let line = |x1, y1, x2, y2| {
                self.target.DrawLine(
                    Vector2 { X: x1, Y: y1 },
                    Vector2 { X: x2, Y: y2 },
                    &brush,
                    1.8,
                    None,
                )
            };
            let polygon = |points: &[(f32, f32)]| {
                for pair in points.windows(2) {
                    line(pair[0].0, pair[0].1, pair[1].0, pair[1].1);
                }
                if let (Some(first), Some(last)) = (points.first(), points.last()) {
                    line(last.0, last.1, first.0, first.1);
                }
            };
            match name {
                "select" => polygon(&[
                    (left + 2.0, top + 1.0),
                    (right - 2.0, cy),
                    (cx, cy + 2.0),
                    (cx - 2.0, bottom - 1.0),
                ]),
                "crop" => {
                    line(left + 3.0, top, left + 3.0, bottom - 3.0);
                    line(left, top + 3.0, right - 3.0, top + 3.0);
                    line(right - 3.0, top + 3.0, right - 3.0, bottom);
                    line(left + 3.0, bottom - 3.0, right, bottom - 3.0);
                }
                "text" => {
                    line(left + 2.0, top + 2.0, right - 2.0, top + 2.0);
                    line(cx, top + 2.0, cx, bottom - 2.0);
                    line(cx - 4.0, bottom - 2.0, cx + 4.0, bottom - 2.0);
                }
                "pen" => {
                    line(left, bottom - 3.0, cx - 3.0, cy - 2.0);
                    line(cx - 3.0, cy - 2.0, cx + 2.0, cy + 4.0);
                    line(cx + 2.0, cy + 4.0, right, top + 2.0);
                    line(left, bottom, right, bottom);
                }
                "eraser" => {
                    polygon(&[
                        (left + 3.0, bottom - 5.0),
                        (cx - 2.0, top + 2.0),
                        (right - 1.0, cy - 2.0),
                        (cx + 3.0, bottom - 5.0),
                    ]);
                    line(left, bottom - 2.0, right, bottom - 2.0);
                }
                "shapes" => {
                    self.target.DrawRectangle(
                        &D2D_RECT_F {
                            left,
                            top: cy - 2.0,
                            right: cx + 2.0,
                            bottom,
                        },
                        &brush,
                        1.8,
                        None,
                    );
                    self.target.DrawEllipse(
                        &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                            point: Vector2 {
                                X: right - 5.0,
                                Y: top + 6.0,
                            },
                            radiusX: 6.0,
                            radiusY: 6.0,
                        },
                        &brush,
                        1.8,
                        None,
                    );
                }
                "arrow" => {
                    line(left, bottom, right, top);
                    line(cx + 2.0, top, right, top);
                    line(right, top, right, cy - 2.0);
                }
                "line" => line(left, bottom, right, top),
                "rectangle" => self.target.DrawRectangle(
                    &D2D_RECT_F {
                        left,
                        top: top + 2.0,
                        right,
                        bottom: bottom - 2.0,
                    },
                    &brush,
                    1.8,
                    None,
                ),
                "ellipse" | "eye" | "eye-off" => {
                    self.target.DrawEllipse(
                        &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                            point: Vector2 { X: cx, Y: cy },
                            radiusX: rect.width / 2.0,
                            radiusY: rect.height / 3.0,
                        },
                        &brush,
                        1.8,
                        None,
                    );
                    if name == "eye" {
                        self.target.FillEllipse(
                            &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                                point: Vector2 { X: cx, Y: cy },
                                radiusX: 2.5,
                                radiusY: 2.5,
                            },
                            &brush,
                        );
                    } else if name == "eye-off" {
                        line(left, top, right, bottom);
                    }
                }
                "triangle" => polygon(&[(cx, top), (right, bottom), (left, bottom)]),
                "diamond" => polygon(&[(cx, top), (right, cy), (cx, bottom), (left, cy)]),
                "star" => {
                    let points = (0..10)
                        .map(|index| {
                            let angle = (-90.0 + index as f32 * 36.0).to_radians();
                            let radius = if index % 2 == 0 { 1.0 } else { 0.42 };
                            (
                                cx + angle.cos() * rect.width / 2.0 * radius,
                                cy + angle.sin() * rect.height / 2.0 * radius,
                            )
                        })
                        .collect::<Vec<_>>();
                    polygon(&points);
                }
                "undo" | "redo" => {
                    let reverse = name == "undo";
                    let from = if reverse { right } else { left };
                    let to = if reverse { left } else { right };
                    line(from, cy, to + if reverse { 4.0 } else { -4.0 }, cy);
                    line(to, cy, to + if reverse { 5.0 } else { -5.0 }, top + 4.0);
                    line(to, cy, to + if reverse { 5.0 } else { -5.0 }, bottom - 4.0);
                }
                "minus" => line(left + 2.0, cy, right - 2.0, cy),
                "plus" => {
                    line(left + 2.0, cy, right - 2.0, cy);
                    line(cx, top + 2.0, cx, bottom - 2.0);
                }
                "chevron-down" => {
                    line(left + 2.0, cy - 3.0, cx, cy + 2.0);
                    line(cx, cy + 2.0, right - 2.0, cy - 3.0);
                }
                "fit" => {
                    for (a, b) in [
                        ((left, top + 5.0), (left, top)),
                        ((left, top), (left + 5.0, top)),
                        ((right - 5.0, top), (right, top)),
                        ((right, top), (right, top + 5.0)),
                        ((right, bottom - 5.0), (right, bottom)),
                        ((right, bottom), (right - 5.0, bottom)),
                        ((left + 5.0, bottom), (left, bottom)),
                        ((left, bottom), (left, bottom - 5.0)),
                    ] {
                        line(a.0, a.1, b.0, b.1);
                    }
                }
                "image" => {
                    self.target
                        .DrawRectangle(&to_d2d(rect.inset(1.0)), &brush, 1.8, None);
                    self.target.FillEllipse(
                        &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                            point: Vector2 {
                                X: left + rect.width * 0.7,
                                Y: top + rect.height * 0.3,
                            },
                            radiusX: 2.0,
                            radiusY: 2.0,
                        },
                        &brush,
                    );
                    line(left + 2.0, bottom - 3.0, cx - 1.0, cy);
                    line(cx - 1.0, cy, right - 2.0, bottom - 3.0);
                }
                "copy" => {
                    self.target.DrawRectangle(
                        &D2D_RECT_F {
                            left: left + 1.0,
                            top: top + 1.0,
                            right: right - 4.0,
                            bottom: bottom - 4.0,
                        },
                        &brush,
                        1.8,
                        None,
                    );
                    self.target.DrawRectangle(
                        &D2D_RECT_F {
                            left: left + 5.0,
                            top: top + 5.0,
                            right: right - 1.0,
                            bottom: bottom - 1.0,
                        },
                        &brush,
                        1.8,
                        None,
                    );
                }
                "save" => {
                    self.target
                        .DrawRectangle(&to_d2d(rect.inset(1.0)), &brush, 1.8, None);
                    self.target.DrawRectangle(
                        &D2D_RECT_F {
                            left: left + 4.0,
                            top: top + 2.0,
                            right: right - 4.0,
                            bottom: cy,
                        },
                        &brush,
                        1.5,
                        None,
                    );
                    line(left + 4.0, bottom - 4.0, right - 4.0, bottom - 4.0);
                }
                "lock" => {
                    self.target.DrawRectangle(
                        &D2D_RECT_F {
                            left: left + 3.0,
                            top: cy,
                            right: right - 3.0,
                            bottom: bottom - 1.0,
                        },
                        &brush,
                        1.8,
                        None,
                    );
                    line(left + 5.0, cy, left + 5.0, top + 5.0);
                    line(left + 5.0, top + 5.0, right - 5.0, top + 5.0);
                    line(right - 5.0, top + 5.0, right - 5.0, cy);
                }
                "unlock" => {
                    self.target.DrawRectangle(
                        &D2D_RECT_F {
                            left: left + 3.0,
                            top: cy,
                            right: right - 3.0,
                            bottom: bottom - 1.0,
                        },
                        &brush,
                        1.8,
                        None,
                    );
                    line(left + 5.0, cy, left + 5.0, top + 6.0);
                    line(left + 5.0, top + 6.0, right - 7.0, top + 3.0);
                    line(right - 7.0, top + 3.0, right - 4.0, top + 7.0);
                }
                "more" => {
                    for offset in [-5.0, 0.0, 5.0] {
                        self.target.FillEllipse(
                            &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                                point: Vector2 {
                                    X: cx,
                                    Y: cy + offset,
                                },
                                radiusX: 1.5,
                                radiusY: 1.5,
                            },
                            &brush,
                        );
                    }
                }
                "play" => polygon(&[
                    (left + 3.0, top + 1.0),
                    (right - 1.0, cy),
                    (left + 3.0, bottom - 1.0),
                ]),
                "pause" => {
                    line(left + 4.0, top + 1.0, left + 4.0, bottom - 1.0);
                    line(right - 4.0, top + 1.0, right - 4.0, bottom - 1.0);
                }
                "window" => {
                    self.target
                        .DrawRectangle(&to_d2d(rect.inset(1.0)), &brush, 1.8, None);
                    line(left + 1.0, top + 5.0, right - 1.0, top + 5.0);
                }
                "display" => {
                    self.target.DrawRectangle(
                        &D2D_RECT_F {
                            left: left + 1.0,
                            top: top + 1.0,
                            right: right - 1.0,
                            bottom: bottom - 4.0,
                        },
                        &brush,
                        1.8,
                        None,
                    );
                    line(cx, bottom - 4.0, cx, bottom - 1.0);
                    line(cx - 4.0, bottom - 1.0, cx + 4.0, bottom - 1.0);
                }
                "video" => {
                    self.target.DrawRectangle(
                        &D2D_RECT_F {
                            left,
                            top: top + 3.0,
                            right: right - 5.0,
                            bottom: bottom - 3.0,
                        },
                        &brush,
                        1.8,
                        None,
                    );
                    polygon(&[
                        (right - 5.0, cy - 3.0),
                        (right, cy - 6.0),
                        (right, cy + 6.0),
                        (right - 5.0, cy + 3.0),
                    ]);
                }
                "clicks" => {
                    for radius in [3.0, 7.0] {
                        self.target.DrawEllipse(
                            &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                                point: Vector2 { X: cx, Y: cy },
                                radiusX: radius,
                                radiusY: radius,
                            },
                            &brush,
                            1.5,
                            None,
                        );
                    }
                }
                "keys" => {
                    self.target.DrawRoundedRectangle(
                        &windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
                            rect: to_d2d(rect.inset(1.0)),
                            radiusX: 2.0,
                            radiusY: 2.0,
                        },
                        &brush,
                        1.8,
                        None,
                    );
                    for offset in [4.0, 8.0, 12.0] {
                        line(left + offset, cy, left + offset + 2.0, cy);
                    }
                }
                "audio" => {
                    polygon(&[
                        (left + 1.0, cy - 3.0),
                        (left + 5.0, cy - 3.0),
                        (cx, top + 2.0),
                        (cx, bottom - 2.0),
                        (left + 5.0, cy + 3.0),
                        (left + 1.0, cy + 3.0),
                    ]);
                    line(cx + 3.0, cy - 4.0, right - 1.0, cy);
                    line(right - 1.0, cy, cx + 3.0, cy + 4.0);
                }
                "trash" => {
                    self.target.DrawRectangle(
                        &D2D_RECT_F {
                            left: left + 4.0,
                            top: top + 6.0,
                            right: right - 4.0,
                            bottom: bottom - 1.0,
                        },
                        &brush,
                        1.7,
                        None,
                    );
                    line(left + 2.0, top + 4.0, right - 2.0, top + 4.0);
                    line(cx - 3.0, top + 1.0, cx + 3.0, top + 1.0);
                }
                _ => line(left, top, right, bottom),
            }
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

    unsafe fn bitmap_contain(&mut self, image: &RgbaImage, destination: Rect) -> Result<()> {
        unsafe {
            self.ensure_bitmap(image)?;
            self.target.DrawBitmap(
                self.image.as_ref().unwrap(),
                Some(&to_d2d(contain(
                    (image.width(), image.height()),
                    destination,
                ))),
                1.0,
                D2D1_INTERPOLATION_MODE_LINEAR,
                None,
                None,
            );
            Ok(())
        }
    }

    unsafe fn bitmap_rect(&mut self, image: &RgbaImage, destination: Rect) -> Result<()> {
        unsafe {
            self.ensure_bitmap(image)?;
            self.target.DrawBitmap(
                self.image.as_ref().unwrap(),
                Some(&to_d2d(destination)),
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

fn shape_icon(shape: &captures_windows_native::editor::Shape) -> &'static str {
    use captures_windows_native::editor::Shape;
    match shape {
        Shape::Stroke(_) => "pen",
        Shape::Arrow(_, _) => "arrow",
        Shape::Line(_, _) => "line",
        Shape::Rectangle(_) => "rectangle",
        Shape::Ellipse(_) => "ellipse",
        Shape::Polygon(_) => "shapes",
        Shape::Image { .. } => "image",
        Shape::Text { .. } => "text",
    }
}

fn blend_mode_label(mode: captures_windows_native::editor::BlendMode) -> &'static str {
    match mode {
        captures_windows_native::editor::BlendMode::Normal => "Normal",
        captures_windows_native::editor::BlendMode::Multiply => "Multiply",
        captures_windows_native::editor::BlendMode::Screen => "Screen",
        captures_windows_native::editor::BlendMode::Overlay => "Overlay",
        captures_windows_native::editor::BlendMode::Darken => "Darken",
        captures_windows_native::editor::BlendMode::Lighten => "Lighten",
    }
}

fn tool_label(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "Select",
        Tool::Crop => "Crop",
        Tool::Text => "Text",
        Tool::Pen => "Freehand",
        Tool::Arrow => "Arrow",
        Tool::Line => "Line",
        Tool::Rectangle => "Rectangle",
        Tool::Ellipse => "Ellipse",
        Tool::Triangle => "Triangle",
        Tool::Diamond => "Diamond",
        Tool::Star => "Star",
        Tool::Eraser => "Eraser",
    }
}

fn contrast_ink(background: Color) -> Color {
    let brightness = (u32::from(background.0) * 299
        + u32::from(background.1) * 587
        + u32::from(background.2) * 114)
        / 1_000;
    if brightness > 150 {
        Color(23, 24, 27, 255)
    } else {
        Color(250, 250, 252, 255)
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
