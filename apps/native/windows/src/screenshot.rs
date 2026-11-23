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
        Window::Feedback => (
            captures_app::feedback::WINDOW_WIDTH,
            captures_app::feedback::WINDOW_HEIGHT,
        ),
        Window::Setup => {
            let spec = captures_app::app_windows::SETUP;
            (spec.width, spec.height)
        }
        Window::Update => (720., 720.),
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
        // `--select recording-locked` draws the bar a running take opens
        // (Record disabled).
        menu.recording_locked = options.select.as_deref() == Some("recording-locked");
        // `--select record` draws the bar in Record; `record-fps` and
        // `record-microphone` also open that select.
        if let Some(select) = options.select.as_deref()
            && !menu.recording_locked
            && select.starts_with("record")
        {
            use crate::capture::Field;
            menu.mode = captures_app::capture_menu::MenuMode::Recording;
            menu.controls_excluded = true;
            menu.record = crate::capture::RecordOptions::new(&settings);
            menu.record.set_microphones(vec![
                ("default".into(), "Microphone (Realtek(R) Audio)".into()),
                ("usb".into(), "Headset Microphone (USB Audio)".into()),
            ]);
            let field = match select {
                "record-fps" => Some(Field::Fps),
                "record-microphone" => Some(Field::Microphone),
                _ => None,
            };
            menu.open = field.map(|field| {
                let state = captures_app::controls::select::State {
                    open: true,
                    active: 1,
                };
                (field, state)
            });
        }
        if target == crate::capture::Target::Region && options.select.is_some() {
            menu.selection = Some(crate::gfx::rect(320., 180., 860., 500.));
        }
        // `--select aspect` opens the Aspect select on 16 : 9; `aspect-top`
        // also moves the region to the top edge (its size label inside).
        if let Some(select @ ("aspect" | "aspect-top")) = options.select.as_deref() {
            menu.aspect = 4;
            if select == "aspect-top" {
                menu.selection = Some(crate::gfx::rect(320., 10., 860., 314.));
            } else {
                let state = captures_app::controls::select::State {
                    open: true,
                    active: 4,
                };
                menu.open = Some((crate::capture::Field::Aspect, state));
            }
        }
        // `--tool display --select display` opens the Display select over two
        // displays; `display-switching` draws the bar while one prepares.
        if let Some(select @ ("display" | "display-switching")) = options.select.as_deref() {
            menu.displays = vec![
                (
                    "1".into(),
                    "Generic PnP Monitor".into(),
                    format!("{} × {} · Primary", w * 2, h * 2),
                ),
                ("2".into(), "DELL U2723QE".into(), "3840 × 2160".into()),
            ];
            if select == "display" {
                let state = captures_app::controls::select::State {
                    open: true,
                    active: 1,
                };
                menu.open = Some((crate::capture::Field::Display, state));
            } else {
                menu.switching = true;
            }
        }
        // `--select direct` draws the bar-less Screenshot Region or Window
        // selector; `direct-feedback` the Region one after an empty click.
        if let Some(select @ ("direct" | "direct-feedback")) = options.select.as_deref() {
            menu.direct = true;
            menu.selection = None;
            if select == "direct-feedback" {
                menu.selection_feedback();
            }
        }
        // The fixture window below; `--select` hovers it, `--select desktop`
        // the display.
        menu.windows = vec![(
            crate::gfx::rect(280., 140., 900., 540.),
            "Notes — Meeting agenda".into(),
        )];
        // `--select window-unavailable` draws Region with no windows listed:
        // Window disabled under its glass tooltip.
        if options.select.as_deref() == Some("window-unavailable") {
            menu.selection = None;
            menu.without_windows_tooltip();
        }
        // `--select focus-segment|focus-select|focus-close|focus-primary|
        // focus-window|focus-link` and `record-focus` draw that control's
        // keyboard focus ring (Show cursor's in Record).
        match options.select.as_deref() {
            Some(select) if select.starts_with("focus-") => menu.show_focus(&select[6..]),
            Some("record-focus") => menu.show_focus("toggle"),
            _ => {}
        }
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
            let stack = crate::previews::fixture(&g, &desktop, options.select.as_deref());
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
            // corner over the desktop; `--select paused|failed|hover|included|
            // tooltip-hide`.
            let mut hud = crate::hud::Hud::new(options.select.as_deref() != Some("included"));
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
                hud.tooltip = crate::tooltip::Fade::settled(hud.hover);
            }
            // `--select tooltip-hide` shows a right-aligned tooltip.
            if options.select.as_deref() == Some("tooltip-hide") {
                hud.tooltip =
                    crate::tooltip::Fade::settled(Some(captures_app::recording_hud::Control::Hide));
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
        } else if options.tool.as_deref() == Some("hidden-notice") {
            // The notice over the desktop with the default New Capture keys.
            let copy = captures_app::recording_hud::hidden_notice_copy(
                &settings.new_capture_shortcut,
                captures_app::shortcuts::ShortcutPlatform::Windows,
            );
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
                crate::hud::paint_hidden_notice(dc, &g, &tokens, &copy)
            })
        } else if options.tool.as_deref() == Some("recording-ready") {
            // The Recording ready notice in the top-right corner; `--select
            // saved|saving|error|hover|dismiss-hover|arriving`.
            use crate::recording_ready::{self as ready, Part, State};
            let select = options.select.as_deref().unwrap_or("");
            let state = State {
                saved: matches!(select, "saved" | "opening")
                    .then(|| r"C:\Users\you\Videos\Captures\Recording.mp4".into()),
                busy: matches!(select, "saving" | "opening"),
                error: (select == "error").then(|| {
                    "Could not save the recording: the Captures folder is not writable".into()
                }),
            };
            let hover = match select {
                "hover" => Some(Part::Action),
                "dismiss-hover" => Some(Part::Dismiss),
                _ => None,
            };
            // `arriving`: 120 ms into `recording-saved-lifecycle`.
            let pose = if select == "arriving" {
                captures_app::motion::Motion::RecordingSavedLifecycle
                    .resolve(&tokens)
                    .map_or(captures_app::motion::Pose::REST, |a| {
                        a.lifecycle_pose(120., Some(14_680.), false)
                    })
            } else {
                captures_app::motion::Pose::REST
            };
            let size = (w as f32, h as f32);
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
                let at =
                    windows_numerics::Matrix3x2::translation(size.0 - ready::SIZE.0 - 18., 30.);
                dc.SetTransform(&(at * base));
                let result = ready::paint(dc, &g, &tokens, &state, hover, pose).map(|_| ());
                dc.SetTransform(&base);
                result
            })
        } else if options.tool.as_deref() == Some("startup-notice") {
            // The launch notice over the desktop: anchored above a tray icon
            // at the bottom right, or `--select fallback` without one.
            use captures_app::tray_notice;
            let screen = tray_notice::LogicalRect::new(0., 0., w as f64, h as f64);
            let work = tray_notice::LogicalRect::new(0., 0., w as f64, h as f64 - 48.);
            let tray = (options.select.as_deref() != Some("fallback"))
                .then(|| tray_notice::LogicalRect::new(w as f64 - 160., h as f64 - 44., 24., 40.));
            let placement = tray_notice::startup_notice_placement(
                screen,
                work,
                tray,
                false,
                tray_notice::FallbackEdge::Bottom,
            );
            let keys = captures_app::shortcuts::shortcut_display_tokens(
                &captures_settings::default_new_capture_shortcut(),
                captures_app::shortcuts::ShortcutPlatform::Windows,
            );
            let hover = options.select.as_deref() == Some("hover");
            let size = (w as f32, h as f32);
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
                let at = windows_numerics::Matrix3x2::translation(
                    placement.x as f32,
                    placement.y as f32,
                );
                dc.SetTransform(&(at * base));
                let result =
                    crate::startup_notice::paint(dc, &g, &tokens, &placement, &keys, hover)
                        .map(|_| ());
                dc.SetTransform(&base);
                result
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
    } else if options.scene == Window::Update {
        let paint = crate::update_notice::render_offscreen(
            options,
            g.clone(),
            tokens,
            (w as f32, h as f32),
        )?;
        Box::new(paint)
    } else if options.scene == Window::Feedback {
        let feedback =
            crate::feedback::render_offscreen(options, g.clone(), tokens, (w as f32, h as f32))?;
        Box::new(move |dc| feedback.paint(dc))
    } else if options.scene == Window::Setup {
        let setup =
            crate::setup::render_offscreen(options, g.clone(), tokens, (w as f32, h as f32))?;
        Box::new(move |dc| setup.paint(dc).map(|_| ()))
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
        // `--select update-STATE` draws a Preview build's Updates card.
        if let Some(card) = options.select.as_deref().and_then(crate::updates::fixture) {
            scene.set_update(Some(card));
            scene.layout(&g, w as f32, h as f32)?;
            scene.activate(crate::scene::Target::Nav(crate::scene::UPDATES_SECTION));
        }
        scene.layout(&g, w as f32, h as f32)?;
        // `--select auto-start-on-selection` (a capture-bar link's target id)
        // draws that row revealed and highlighted.
        if let Some(target) = options
            .select
            .as_deref()
            .and_then(captures_app::capture_menu::PreferenceTarget::from_id)
        {
            scene.reveal_target(target);
        }
        if let Some(query) = &options.find {
            // As if Ctrl+F was pressed and the query typed.
            scene.find_reveal(&g);
            scene.layout(&g, w as f32, h as f32)?;
            scene.field_edit(&g, |editor| editor.set_text(query));
            scene.prepare(&g)?;
        }
        let g = g.clone();
        // `--select color-picker` draws the custom-color picker (with
        // opacity) over the page; `color-picker-opaque` the theme colors' one.
        let picker = match options.select.as_deref() {
            Some("color-picker") => Some(crate::color_picker::Picker::new("#bfff8080", true)),
            Some("color-picker-opaque") => Some(crate::color_picker::Picker::new("#32d3ff", false)),
            _ => None,
        };
        let t = scene.t.clone();
        Box::new(move |dc| {
            scene.paint(dc, &g, 1.)?;
            match &picker {
                Some(picker) => picker.paint(dc, &t, (260., 200.), 1.),
                None => Ok(()),
            }
        })
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
