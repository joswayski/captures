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
        Window::Recording => crate::recording::SIZE,
        Window::Capture => (1280., 800.),
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
    } else if options.scene == Window::Capture {
        let target = match options.tool.as_deref() {
            Some("display") => crate::capture::Target::Display,
            Some("window") => crate::capture::Target::Window,
            _ => crate::capture::Target::Region,
        };
        let mut menu = crate::capture::Menu::new(
            target,
            (w as f32, h as f32),
            ("Generic PnP Monitor".into(), w * 2, h * 2),
            settings.auto_start_on_selection,
        );
        // `--select record` draws the bar in Record.
        if options.select.as_deref() == Some("record") {
            menu.mode = captures_app::capture_menu::MenuMode::Recording;
            menu.controls_excluded = true;
        }
        if target == crate::capture::Target::Region && options.select.is_some() {
            menu.selection = Some(crate::gfx::rect(320., 180., 860., 500.));
        }
        // The fixture window below; `--select` hovers it, `--select desktop`
        // the display.
        menu.windows = vec![(
            crate::gfx::rect(280., 140., 900., 540.),
            "Notes — Meeting agenda".into(),
        )];
        if target == crate::capture::Target::Window {
            menu.hovered = match options.select.as_deref() {
                Some("desktop" | "picked") => Some(None),
                Some(_) => Some(Some(0)),
                None => None,
            };
            if options.select.as_deref() == Some("picked") {
                menu.picked = Some(Some(0));
            }
        }
        // A synthetic desktop: a diagonal gradient with a light window.
        let desktop = image::RgbaImage::from_fn(w, h, |x, y| {
            let inside = (280..900).contains(&x) && (140..540).contains(&y);
            let v = ((x + y) * 255 / (w + h)) as u8;
            if inside {
                image::Rgba([236, 236, 232, 255])
            } else {
                image::Rgba([40 + v / 3, 70 + v / 4, 120 + v / 3, 255])
            }
        });
        let frozen = crate::capture::bitmap(&g, &desktop)?;
        let menu = std::cell::RefCell::new(menu);
        let g = g.clone();
        if options.tool.as_deref() == Some("previews") {
            // The bottom-left stack over the desktop, as the 1280×800 mock.
            let stack = crate::previews::fixture(&g, &desktop, options.select.is_some());
            let size = (w as f32, h as f32);
            let top = size.1 - stack.height;
            Box::new(move |dc| unsafe {
                dc.DrawBitmap(
                    &frozen,
                    Some(&crate::gfx::rect(0., 0., size.0, size.1)),
                    1.,
                    windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
                let mut base = windows_numerics::Matrix3x2::identity();
                dc.GetTransform(&mut base);
                dc.SetTransform(&(windows_numerics::Matrix3x2::translation(0., top) * base));
                let result = stack.paint(dc, &g, &tokens);
                dc.SetTransform(&base);
                result
            })
        } else if options.tool.as_deref() == Some("hud") {
            // The recording controls at 1:34 with a microphone, at the canvas
            // corner over the desktop; `--select paused|failed|hover`.
            let mut hud = crate::hud::Hud::new(true);
            hud.input.busy = false;
            hud.input.has_microphone = true;
            hud.elapsed_ms = 94_000;
            hud.level = 0.0;
            hud.input.state = match options.select.as_deref() {
                Some("paused") => captures_recording::RecordingState::Paused,
                Some("failed") => captures_recording::RecordingState::Failed,
                _ => captures_recording::RecordingState::Recording,
            };
            if options.select.as_deref() == Some("failed") {
                hud.error = Some("ffmpeg exited before the first frame".into());
            }
            if options.select.as_deref() == Some("hover") {
                hud.hover = Some(captures_app::recording_hud::Control::Stop);
            }
            let size = (w as f32, h as f32);
            Box::new(move |dc| {
                unsafe {
                    dc.DrawBitmap(
                        &frozen,
                        Some(&crate::gfx::rect(0., 0., size.0, size.1)),
                        1.,
                        windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_LINEAR,
                        None,
                        None,
                    )
                };
                hud.paint(dc, &g, &tokens, 0.)
            })
        } else if options.tool.as_deref() == Some("countdown") {
            // The scrim over the live desktop at 3, at rest.
            let size = (w as f32, h as f32);
            Box::new(move |dc| {
                unsafe {
                    dc.DrawBitmap(
                        &frozen,
                        Some(&crate::gfx::rect(0., 0., size.0, size.1)),
                        1.,
                        windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_LINEAR,
                        None,
                        None,
                    )
                };
                let rest = captures_app::motion::Pose::REST;
                crate::countdown::paint(
                    dc,
                    &g,
                    &tokens,
                    size,
                    3,
                    ("SCREENSHOT IN", "Press Esc to cancel"),
                    rest,
                    rest,
                )
            })
        } else {
            Box::new(move |dc| menu.borrow_mut().paint(dc, &g, &tokens, Some(&frozen)))
        }
    } else if options.scene == Window::Recording {
        let recording =
            crate::recording::render_offscreen(options, g.clone(), tokens, (w as f32, h as f32))?;
        Box::new(move |dc| recording.paint(dc))
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
