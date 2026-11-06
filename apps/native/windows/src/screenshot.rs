//! `--screenshot`: paint one frame of Preferences, History or the editor into a Direct2D
//! bitmap and encode it as PNG through WIC, without opening a window.
use windows::Win32::Foundation::{E_FAIL, GENERIC_WRITE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::core::*;

use crate::Scene as Window;
use crate::gfx::Gfx;
use crate::scene::Scene;
use crate::{Options, appearance};

/// Paints one page into a device context.
type Paint = dyn Fn(&ID2D1DeviceContext) -> Result<()>;

pub fn render(options: &Options, path: &std::path::Path) -> Result<()> {
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let tokens = appearance::tokens(options.appearance.as_deref(), &settings);
    let g = Gfx::new(&tokens, false)?;
    let (w, h) = match options.scene {
        Window::Preferences => {
            let spec = captures_app::app_windows::PREFERENCES;
            (spec.width, spec.height)
        }
        Window::History => {
            let spec = captures_app::app_windows::HISTORY;
            (spec.width, spec.height)
        }
        Window::Editor => crate::editor::SIZE,
    };
    let (w, h) = options.size.unwrap_or((w as u32, h as u32));
    let canvas = tokens.color("surface-canvas");
    let page: Box<Paint> = if options.scene == Window::History {
        let history =
            crate::history::render_offscreen(options, g.clone(), tokens, (w as f32, h as f32))?;
        Box::new(move |dc| history.paint(dc, 1.))
    } else if options.scene == Window::Editor {
        let editor =
            crate::editor::render_offscreen(options, g.clone(), tokens, (w as f32, h as f32))?;
        Box::new(move |dc| editor.paint(dc, 1.))
    } else {
        let platform = captures_app::shortcuts::ShortcutPlatform::Windows;
        let detail = match &options.history_root {
            Some(_) => captures_app::preferences::LOGIN_ITEM_CHECKING,
            None => captures_app::preferences::login_item_unavailable(platform),
        };
        let mut scene = Scene::new(tokens, options.settings_file.clone(), detail.into());
        scene.layout(&g, w as f32, h as f32)?;
        if let Some(query) = &options.find {
            // As if Ctrl+F was pressed and the query typed.
            scene.find_reveal(&g);
            scene.layout(&g, w as f32, h as f32)?;
            scene.field_edit(&g, |editor| editor.set_text(query));
            scene.prepare(&g)?;
        }
        let g = g.clone();
        Box::new(move |dc| scene.paint(dc, &g, 1.))
    };
    unsafe {
        let props = |options| D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.,
            dpiY: 96.,
            bitmapOptions: options,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let size = D2D_SIZE_U {
            width: w,
            height: h,
        };
        let target =
            g.dc.CreateBitmap(size, None, 0, &props(D2D1_BITMAP_OPTIONS_TARGET))?;
        g.dc.SetTarget(&target);
        g.dc.SetDpi(96., 96.);
        g.dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        g.dc.BeginDraw();
        g.dc.Clear(Some(&canvas));
        let painted = page(&g.dc);
        g.dc.EndDraw(None, None)?;
        painted?;
        g.dc.SetTarget(None);
        let read = g.dc.CreateBitmap(
            size,
            None,
            0,
            &props(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW),
        )?;
        read.CopyFromBitmap(None, &target, None)?;
        let mapped = read.Map(D2D1_MAP_OPTIONS_READ)?;
        let bytes = std::slice::from_raw_parts(mapped.bits, (mapped.pitch * h) as usize);
        let encoded = encode(path, w, h, mapped.pitch, bytes);
        read.Unmap()?;
        encoded
    }
}

/// The frame is opaque (cleared to the canvas), so premultiplied BGRA is
/// already straight BGRA.
unsafe fn encode(path: &std::path::Path, w: u32, h: u32, pitch: u32, bytes: &[u8]) -> Result<()> {
    unsafe {
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
        let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        let mut bag = None;
        encoder.CreateNewFrame(&mut frame, &mut bag)?;
        let frame = frame.ok_or_else(Error::empty)?;
        frame.Initialize(bag.as_ref())?;
        frame.SetSize(w, h)?;
        let mut format = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format)?;
        if format != GUID_WICPixelFormat32bppBGRA {
            return Err(Error::new(E_FAIL, "PNG encoder rejected 32bpp BGRA"));
        }
        frame.WritePixels(h, pitch, bytes)?;
        frame.Commit()?;
        encoder.Commit()
    }
}
