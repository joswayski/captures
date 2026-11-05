//! Devices and drawing primitives: D3D11 → DXGI → Direct2D device context, the
//! DirectComposition desktop device, and DirectWrite on the system font
//! collection (Captures' `--font-sans` resolves to Segoe UI Variable Text on
//! Windows 11 and Segoe UI before it).
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::tokens::{Shadow, Tokens};

/// Text roles in the Preferences window, each a shipping CSS rule.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// `.preferences-nav-brand strong`
    Brand,
    /// `.preferences-nav nav button`
    Nav,
    /// `.preferences-header h1`
    Title,
    /// `.preferences-header > div > p`
    Subtitle,
    /// `.preferences-history-button`
    Button,
    /// `.settings-card-header h2`
    CardTitle,
    /// `.settings-card-header p`
    CardDescription,
    /// `.check-row.switch-row > span`
    RowTitle,
    /// `.check-row small`
    RowDescription,
}

const STYLES: [Style; 9] = [
    Style::Brand,
    Style::Nav,
    Style::Title,
    Style::Subtitle,
    Style::Button,
    Style::CardTitle,
    Style::CardDescription,
    Style::RowTitle,
    Style::RowDescription,
];

struct Spec {
    size: &'static str,
    weight: &'static str,
    tracking: Option<&'static str>,
    leading: Option<&'static str>,
    wrap: bool,
}

fn spec(style: Style) -> Spec {
    let spec = |size, weight, tracking, leading, wrap| Spec {
        size,
        weight,
        tracking,
        leading,
        wrap,
    };
    match style {
        Style::Brand => spec(
            "text-md",
            "weight-semibold",
            Some("tracking-tight"),
            None,
            false,
        ),
        Style::Nav => spec("text-md", "weight-medium", None, None, false),
        Style::Title => spec(
            "text-xl",
            "weight-semibold",
            Some("tracking-tighter"),
            Some("leading-tight"),
            false,
        ),
        Style::Subtitle => spec("text-sm", "weight-regular", None, None, false),
        Style::Button => spec("text-sm", "weight-medium", None, None, false),
        Style::CardTitle => spec(
            "text-lg",
            "weight-semibold",
            Some("tracking-tight"),
            Some("leading-tight"),
            true,
        ),
        Style::CardDescription | Style::RowDescription => spec(
            "text-sm",
            "weight-regular",
            None,
            Some("leading-snug"),
            true,
        ),
        Style::RowTitle => spec("text-md", "weight-medium", None, None, true),
    }
}

pub struct Text {
    pub layout: IDWriteTextLayout,
    pub width: f32,
    pub height: f32,
}

pub struct Gfx {
    pub dc: ID2D1DeviceContext,
    pub factory: ID2D1Factory1,
    pub dcomp: Option<IDCompositionDesktopDevice>,
    dwrite: IDWriteFactory,
    formats: Vec<(Style, IDWriteTextFormat)>,
    /// Ascent and descent per em of the UI face, for CSS `line-height`.
    ascent: f32,
    descent: f32,
}

const FAMILIES: [&str; 2] = ["Segoe UI Variable Text", "Segoe UI"];

impl Gfx {
    /// `composition` is false for the offscreen `--screenshot` renderer.
    pub fn new(tokens: &Tokens, composition: bool) -> Result<Self> {
        unsafe {
            let d3d = create_d3d()?;
            let dxgi: IDXGIDevice = d3d.cast()?;
            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d = factory.CreateDevice(&dxgi)?;
            let dc = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let dcomp = if composition {
                Some(DCompositionCreateDevice2(&d2d)?)
            } else {
                None
            };
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let mut collection = None;
            dwrite.GetSystemFontCollection(&mut collection, false)?;
            let collection = collection.ok_or_else(Error::empty)?;
            let mut chosen = None;
            for family in FAMILIES {
                let (mut index, mut exists) = (0, BOOL(0));
                collection.FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)?;
                if exists.as_bool() {
                    chosen = Some((family, index));
                    break;
                }
            }
            let (family, index) = chosen.unwrap_or((FAMILIES[1], 0));
            let mut metrics = DWRITE_FONT_METRICS::default();
            if let Ok(font) = collection.GetFontFamily(index).and_then(|f| {
                f.GetFirstMatchingFont(
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                )
            }) {
                font.GetMetrics(&mut metrics);
            }
            let em = f32::from(metrics.designUnitsPerEm.max(1));
            let (ascent, descent) = if metrics.designUnitsPerEm == 0 {
                (1.07, 0.26)
            } else {
                (
                    f32::from(metrics.ascent) / em,
                    f32::from(metrics.descent) / em,
                )
            };
            // Windows 10 1809+: weights as `wght` axis values, so 500 is a true
            // medium on Segoe UI Variable rather than its nearest named face.
            let axes: Option<IDWriteFactory6> = dwrite.cast().ok();
            let mut formats = Vec::new();
            for style in STYLES {
                let spec = spec(style);
                let (size, weight) = (tokens.n(spec.size), tokens.n(spec.weight));
                let family = &HSTRING::from(family);
                let format: IDWriteTextFormat = match axes.as_ref().and_then(|f| {
                    f.CreateTextFormat(
                        family,
                        None,
                        &[DWRITE_FONT_AXIS_VALUE {
                            axisTag: DWRITE_FONT_AXIS_TAG_WEIGHT,
                            value: weight,
                        }],
                        size,
                        w!("en-us"),
                    )
                    .ok()
                }) {
                    Some(format) => format.cast()?,
                    None => dwrite.CreateTextFormat(
                        family,
                        None,
                        DWRITE_FONT_WEIGHT(weight as i32),
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        size,
                        w!("en-us"),
                    )?,
                };
                format.SetWordWrapping(if spec.wrap {
                    DWRITE_WORD_WRAPPING_WRAP
                } else {
                    DWRITE_WORD_WRAPPING_NO_WRAP
                })?;
                formats.push((style, format));
            }
            Ok(Self {
                dc,
                factory,
                dcomp,
                dwrite,
                formats,
                ascent,
                descent,
            })
        }
    }

    /// Lay out `text` as `style`, wrapping at `max_width` when the style wraps.
    pub fn text(&self, t: &Tokens, style: Style, text: &str, max_width: f32) -> Result<Text> {
        let spec = spec(style);
        let size = t.n(spec.size);
        let wide: Vec<u16> = text.encode_utf16().collect();
        let format = &self
            .formats
            .iter()
            .find(|(s, _)| *s == style)
            .expect("format")
            .1;
        unsafe {
            let layout =
                self.dwrite
                    .CreateTextLayout(&wide, format, max_width.max(1.), 100_000.)?;
            if let Some(tracking) = spec.tracking {
                // CSS `letter-spacing` adds after every character.
                let layout1: IDWriteTextLayout1 = layout.cast()?;
                layout1.SetCharacterSpacing(
                    0.,
                    t.em(tracking, size),
                    0.,
                    DWRITE_TEXT_RANGE {
                        startPosition: 0,
                        length: wide.len() as u32,
                    },
                )?;
            }
            if let Some(leading) = spec.leading {
                // CSS line boxes split the leading evenly above and below.
                let line = t.n(leading) * size;
                let content = (self.ascent + self.descent) * size;
                layout.SetLineSpacing(
                    DWRITE_LINE_SPACING_METHOD_UNIFORM,
                    line,
                    (line - content) / 2. + self.ascent * size,
                )?;
            }
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics)?;
            Ok(Text {
                layout,
                width: metrics.width,
                height: metrics.height,
            })
        }
    }

    /// The advance of `0` in `style`, for CSS `ch` units.
    pub fn ch(&self, t: &Tokens, style: Style) -> Result<f32> {
        let zeros = self.text(t, style, "0000000000", 100_000.)?;
        Ok(zeros.width / 10.)
    }

    pub fn surface(&self, w: u32, h: u32) -> Result<IDCompositionSurface> {
        let dcomp = self.dcomp.as_ref().ok_or_else(Error::empty)?;
        unsafe {
            dcomp.CreateSurface(
                w.max(1),
                h.max(1),
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_ALPHA_MODE_PREMULTIPLIED,
            )
        }
    }
}

unsafe fn create_d3d() -> Result<ID3D11Device> {
    // Hardware first; WARP keeps headless CI runners and RDP sessions working.
    let mut last = Error::empty();
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let mut device = None;
        let result = unsafe {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[
                    D3D_FEATURE_LEVEL_11_1,
                    D3D_FEATURE_LEVEL_11_0,
                    D3D_FEATURE_LEVEL_10_1,
                ]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        };
        match (result, device) {
            (Ok(()), Some(device)) => return Ok(device),
            (Err(error), _) => last = error,
            _ => {}
        }
    }
    Err(last)
}

/// Draw a whole composition surface of `size` pixels. The closure works in
/// DIPs with its origin `bleed_px` pixels inside the surface's top-left corner.
pub fn draw_surface(
    surface: &IDCompositionSurface,
    size: (u32, u32),
    scale: f32,
    bleed_px: f32,
    f: impl FnOnce(&ID2D1DeviceContext) -> Result<()>,
) -> Result<()> {
    unsafe {
        let mut offset = POINT::default();
        let dc: ID2D1DeviceContext = surface.BeginDraw(None, &mut offset)?;
        dc.SetDpi(96., 96.);
        dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        // Atlased surfaces share a texture: clip to ours before clearing.
        dc.SetTransform(&Matrix3x2::translation(offset.x as f32, offset.y as f32));
        dc.PushAxisAlignedClip(
            &rect(0., 0., size.0 as f32, size.1 as f32),
            D2D1_ANTIALIAS_MODE_ALIASED,
        );
        dc.Clear(Some(&D2D1_COLOR_F::default()));
        dc.SetTransform(&Matrix3x2 {
            M11: scale,
            M12: 0.,
            M21: 0.,
            M22: scale,
            M31: offset.x as f32 + bleed_px,
            M32: offset.y as f32 + bleed_px,
        });
        let result = f(&dc);
        dc.PopAxisAlignedClip();
        surface.EndDraw()?;
        result
    }
}

pub fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}

pub fn brush(dc: &ID2D1DeviceContext, color: D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush> {
    unsafe { dc.CreateSolidColorBrush(&color, None) }
}

fn rounded(r: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: r,
        radiusX: radius,
        radiusY: radius,
    }
}

pub fn fill(dc: &ID2D1DeviceContext, r: D2D_RECT_F, color: D2D1_COLOR_F) -> Result<()> {
    unsafe { dc.FillRectangle(&r, &brush(dc, color)?) };
    Ok(())
}

pub fn fill_round(
    dc: &ID2D1DeviceContext,
    r: D2D_RECT_F,
    radius: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    unsafe { dc.FillRoundedRectangle(&rounded(r, radius), &brush(dc, color)?) };
    Ok(())
}

/// A CSS `border` of `width` drawn inside the border box.
pub fn border(
    dc: &ID2D1DeviceContext,
    r: D2D_RECT_F,
    radius: f32,
    width: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    ring(dc, r, radius, -width, 0., color)
}

/// A band between `from` and `to` DIPs outside `r` (negative is inside), as a
/// CSS `box-shadow: 0 0 0 Npx` focus ring draws it.
pub fn ring(
    dc: &ID2D1DeviceContext,
    r: D2D_RECT_F,
    radius: f32,
    from: f32,
    to: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    let mid = (from + to) / 2.;
    let band = rect(r.left - mid, r.top - mid, r.right + mid, r.bottom + mid);
    unsafe {
        dc.DrawRoundedRectangle(
            &rounded(band, (radius + mid).max(0.)),
            &brush(dc, color)?,
            to - from,
            None,
        )
    };
    Ok(())
}

/// A CSS `box-shadow` stack behind a rounded box. The blur is approximated by
/// stacked translucent outsets (no Gaussian blur effect) so it stays cheap and
/// identical between composition surfaces and screenshots.
pub fn shadow(
    dc: &ID2D1DeviceContext,
    r: D2D_RECT_F,
    radius: f32,
    layers: &[Shadow],
) -> Result<()> {
    for layer in layers.iter().rev() {
        let steps = (layer.blur.ceil() as usize).clamp(1, 8);
        let [cr, cg, cb, ca] = layer.color;
        for step in 0..steps {
            // Outsets from -blur/2 to +blur/2 accumulate a linear falloff.
            let d = layer.spread + layer.blur * ((step as f32 + 0.5) / steps as f32 - 0.5);
            let band = rect(
                r.left + layer.x - d,
                r.top + layer.y - d,
                r.right + layer.x + d,
                r.bottom + layer.y + d,
            );
            fill_round(
                dc,
                band,
                (radius + d).max(0.),
                D2D1_COLOR_F {
                    r: cr,
                    g: cg,
                    b: cb,
                    a: ca / steps as f32,
                },
            )?;
        }
    }
    Ok(())
}

pub fn text(
    dc: &ID2D1DeviceContext,
    text: &Text,
    x: f32,
    y: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    unsafe {
        dc.DrawTextLayout(
            Vector2 { X: x, Y: y },
            &text.layout,
            &brush(dc, color)?,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        )
    };
    Ok(())
}

/// A shipping 24-unit icon at `side` DIPs: every path is stroked except those
/// listed in `filled`, which are filled (`.capture-icon-spark`).
pub fn icon(
    dc: &ID2D1DeviceContext,
    factory: &ID2D1Factory1,
    name: &str,
    filled: &[usize],
    (x, y, side): (f32, f32, f32),
    stroke: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    let Some(paths) = captures_app::icons::paths(name) else {
        return Ok(());
    };
    let scale = side / 24.;
    let brush = brush(dc, color)?;
    unsafe {
        let style = factory.CreateStrokeStyle(
            &D2D1_STROKE_STYLE_PROPERTIES1 {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.,
                ..Default::default()
            },
            None,
        )?;
        for (index, d) in paths.iter().enumerate() {
            let fill = filled.contains(&index);
            let geometry = factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            for line in captures_app::icons::flatten(d) {
                let points: Vec<Vector2> = line
                    .iter()
                    .map(|p| Vector2 {
                        X: x + p[0] * scale,
                        Y: y + p[1] * scale,
                    })
                    .collect();
                let Some((first, rest)) = points.split_first() else {
                    continue;
                };
                sink.BeginFigure(
                    *first,
                    if fill {
                        D2D1_FIGURE_BEGIN_FILLED
                    } else {
                        D2D1_FIGURE_BEGIN_HOLLOW
                    },
                );
                sink.AddLines(rest);
                sink.EndFigure(if fill {
                    D2D1_FIGURE_END_CLOSED
                } else {
                    D2D1_FIGURE_END_OPEN
                });
            }
            sink.Close()?;
            if fill {
                dc.FillGeometry(&geometry, &brush, None);
            } else {
                dc.DrawGeometry(&geometry, &brush, stroke * scale, &style);
            }
        }
    }
    Ok(())
}
