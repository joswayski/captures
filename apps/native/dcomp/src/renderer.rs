//! DirectComposition owns presentation and animation. Direct2D paints only on
//! invalidation; device-dependent resources are discarded together on failure.
use std::{collections::BTreeMap, path::Path};
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND, POINT},
        Graphics::{
            Direct2D::{Common::*, *},
            Direct3D::*,
            Direct3D11::*,
            DirectComposition::*,
            DirectWrite::*,
            Dxgi::{Common::*, *},
        },
    },
    core::{Interface, Result, w},
};
use windows_numerics::Matrix3x2;

use crate::scene::{Node, Rect};

pub struct Renderer {
    device: ID3D11Device,
    desktop: IDCompositionDesktopDevice,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual3,
    surface: IDCompositionSurface,
    write: IDWriteFactory,
    formats: BTreeMap<(u32, bool), IDWriteTextFormat>,
    bitmap: Option<ID2D1Bitmap1>,
    size: [u32; 2],
    pub software: bool,
}

impl Renderer {
    pub fn new(hwnd: HWND, size: [u32; 2]) -> Result<Self> {
        // COM resources are used exclusively on the native event-loop thread.
        unsafe {
            let mut device = None;
            let hardware = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            );
            let software = hardware.is_err();
            if software {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )?;
            }
            let device = device.expect("successful D3D device creation");
            let dxgi: IDXGIDevice = device.cast()?;
            let d2d = D2D1CreateDevice(&dxgi, None)?;
            let desktop: IDCompositionDesktopDevice = DCompositionCreateDevice2(&d2d)?;
            let target = desktop.CreateTargetForHwnd(hwnd, true)?;
            let visual: IDCompositionVisual3 = desktop.CreateVisual()?.cast()?;
            target.SetRoot(&visual)?;
            let surface = desktop.CreateSurface(
                size[0],
                size[1],
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_ALPHA_MODE_PREMULTIPLIED,
            )?;
            visual.SetContent(&surface)?;
            Ok(Self {
                device,
                desktop,
                _target: target,
                visual,
                surface,
                write: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
                formats: BTreeMap::new(),
                bitmap: None,
                size,
                software,
            })
        }
    }

    pub fn resize(&mut self, size: [u32; 2]) -> Result<()> {
        if size == self.size {
            return Ok(());
        }
        unsafe {
            self.surface = self.desktop.CreateSurface(
                size[0],
                size[1],
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_ALPHA_MODE_PREMULTIPLIED,
            )?;
            self.visual.SetContent(&self.surface)?;
        }
        self.size = size;
        Ok(())
    }

    pub fn opacity(&self, fade: bool, reduced_motion: bool, seconds: f32) -> Result<()> {
        unsafe {
            if fade && !reduced_motion {
                let animation = self.desktop.CreateAnimation()?;
                animation.AddCubic(0., 1., -1. / seconds, 0., 0.)?;
                animation.End(seconds as f64, 0.)?;
                self.visual.SetOpacity(&animation)?;
            } else {
                self.visual.SetOpacity2(if fade { 0. } else { 1. })?;
            }
            self.desktop.Commit()
        }
    }

    pub fn paint(&mut self, nodes: &[Node], scale: f32, screenshot: Option<&Path>) -> Result<()> {
        unsafe {
            self.device.GetDeviceRemovedReason()?;
            let mut offset = POINT::default();
            let dc: ID2D1DeviceContext = self.surface.BeginDraw(None, &mut offset)?;
            // EndDraw is required even when text/resource/readback setup fails.
            let result = self.draw(&dc, nodes, scale, offset, screenshot);
            let end = self.surface.EndDraw();
            result?;
            end?;
            self.desktop.Commit()
        }
    }

    fn draw(
        &mut self,
        dc: &ID2D1DeviceContext,
        nodes: &[Node],
        scale: f32,
        offset: POINT,
        screenshot: Option<&Path>,
    ) -> Result<()> {
        unsafe {
            dc.SetDpi(scale * 96., scale * 96.);
            dc.SetTransform(&Matrix3x2::translation(
                offset.x as f32 / scale,
                offset.y as f32 / scale,
            ));
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            dc.Clear(Some(&color([0.; 4])));
            let brush = dc.CreateSolidColorBrush(&color([0., 0., 0., 1.]), None)?;
            for node in nodes {
                match node {
                    Node::PushClip(rect) => {
                        dc.PushAxisAlignedClip(&rectangle(*rect), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE)
                    }
                    Node::PopClip => dc.PopAxisAlignedClip(),
                    Node::Box { rect, fill, radius } => {
                        brush.SetColor(&color(*fill));
                        dc.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT {
                                rect: rectangle(*rect),
                                radiusX: *radius,
                                radiusY: *radius,
                            },
                            &brush,
                        );
                    }
                    Node::Text {
                        rect,
                        text,
                        size,
                        bold,
                        color: ink,
                    } => {
                        let key = (size.to_bits(), *bold);
                        if !self.formats.contains_key(&key) {
                            let format = self.write.CreateTextFormat(
                                w!("Segoe UI"),
                                None,
                                if *bold {
                                    DWRITE_FONT_WEIGHT_SEMI_BOLD
                                } else {
                                    DWRITE_FONT_WEIGHT_NORMAL
                                },
                                DWRITE_FONT_STYLE_NORMAL,
                                DWRITE_FONT_STRETCH_NORMAL,
                                *size,
                                w!("en-US"),
                            )?;
                            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                            self.formats.insert(key, format);
                        }
                        brush.SetColor(&color(*ink));
                        dc.DrawText(
                            &text.encode_utf16().collect::<Vec<_>>(),
                            &self.formats[&key],
                            &rectangle(*rect),
                            &brush,
                            D2D1_DRAW_TEXT_OPTIONS_CLIP,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                    Node::Image { rect } => {
                        if self.bitmap.is_none() {
                            let pixels = fixture();
                            self.bitmap = Some(dc.CreateBitmap(
                                D2D_SIZE_U {
                                    width: 2048,
                                    height: 1152,
                                },
                                Some(pixels.as_ptr().cast()),
                                2048 * 4,
                                &D2D1_BITMAP_PROPERTIES1 {
                                    pixelFormat: D2D1_PIXEL_FORMAT {
                                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                                    },
                                    dpiX: 96.,
                                    dpiY: 96.,
                                    ..Default::default()
                                },
                            )?);
                        }
                        dc.DrawBitmap(
                            self.bitmap.as_ref().unwrap(),
                            Some(&rectangle(*rect)),
                            1.,
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            None,
                            None,
                        );
                    }
                }
            }
            if let Some(path) = screenshot {
                self.readback(dc, offset, path)?;
            }
            Ok(())
        }
    }

    fn readback(&self, dc: &ID2D1DeviceContext, offset: POINT, path: &Path) -> Result<()> {
        unsafe {
            let bitmap = dc.CreateBitmap(
                D2D_SIZE_U {
                    width: self.size[0],
                    height: self.size[1],
                },
                None,
                0,
                &D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    bitmapOptions: D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                    ..Default::default()
                },
            )?;
            bitmap.CopyFromRenderTarget(
                None,
                dc,
                Some(&D2D_RECT_U {
                    left: offset.x as u32,
                    top: offset.y as u32,
                    right: offset.x as u32 + self.size[0],
                    bottom: offset.y as u32 + self.size[1],
                }),
            )?;
            let mapped = bitmap.Map(D2D1_MAP_OPTIONS_READ)?;
            let mut rgba = Vec::with_capacity((self.size[0] * self.size[1] * 4) as usize);
            for y in 0..self.size[1] {
                // D2D owns a mapped allocation of pitch × height until Unmap.
                let row = std::slice::from_raw_parts(
                    mapped.bits.add((y * mapped.pitch) as usize),
                    (self.size[0] * 4) as usize,
                );
                for p in row.chunks_exact(4) {
                    for index in [2, 1, 0] {
                        rgba.push(if p[3] == 0 {
                            0
                        } else {
                            ((u32::from(p[index]) * 255 / u32::from(p[3])).min(255)) as u8
                        });
                    }
                    rgba.push(p[3]);
                }
            }
            bitmap.Unmap()?;
            image::save_buffer(
                path,
                &rgba,
                self.size[0],
                self.size[1],
                image::ColorType::Rgba8,
            )
            .map_err(|error| {
                windows::core::Error::new(
                    windows::core::HRESULT(0x80004005u32 as i32),
                    error.to_string(),
                )
            })
        }
    }
}

fn color([r, g, b, a]: [f32; 4]) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}
fn rectangle(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.x + r.w,
        bottom: r.y + r.h,
    }
}

fn fixture() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(2048 * 1152 * 4);
    for y in 0..1152 {
        for x in 0..2048 {
            let x = x as f32 / 2048. * 284.;
            let y = (1. - y as f32 / 1152.) * 160.;
            let [r, g, b] = if (x - 233.).powi(2) + (y - 123.).powi(2) < 225. {
                [245, 189, 74]
            } else if (38.0..113.).contains(&x) && (50.0..130.).contains(&y) {
                [217, 84, 105]
            } else if y < 45. {
                [51, 122, 102]
            } else {
                [31, 69, 107]
            };
            bytes.extend([b, g, r, 255]);
        }
    }
    bytes
}
