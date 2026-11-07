//! Retained recording preview: one replaceable decoded frame, a cached GPU
//! bitmap, Fit/actual sizing and the shipping playback controls.
use std::cell::RefCell;
use std::sync::{Arc, atomic::Ordering};

use image::RgbaImage;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::Result;
use windows_numerics::Matrix3x2;

use super::*;

pub(super) struct Frame {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

impl Frame {
    /// Convert on the media worker, never while painting a native control.
    pub fn new(image: &RgbaImage) -> Self {
        let mut bgra = Vec::with_capacity(image.as_raw().len());
        for pixel in image.pixels() {
            let [r, g, b, a] = pixel.0;
            let premultiply = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
            bgra.extend([premultiply(b), premultiply(g), premultiply(r), a]);
        }
        Self {
            width: image.width(),
            height: image.height(),
            bgra,
        }
    }
}

#[derive(Default)]
pub(super) struct Preview {
    pub frame: Option<Arc<Frame>>,
    bitmap: RefCell<Option<ID2D1Bitmap1>>,
    pub actual: bool,
    pub scroll: (f32, f32),
    pub playing: bool,
    pub hovered: bool,
    pub generation: u64,
    pub position_ms: u64,
}

impl Recording {
    pub fn target_label(&self, target: Target) -> &'static str {
        if target == Target::Play && self.preview.playing {
            "Pause preview"
        } else {
            target.label()
        }
    }

    pub fn toggled(&self, target: Target) -> Option<bool> {
        match target {
            Target::Copy => Some(self.make_copy),
            Target::Loop => Some(self.worker.looping.load(Ordering::Relaxed)),
            Target::Fit => Some(!self.preview.actual),
            Target::Actual => Some(self.preview.actual),
            _ => None,
        }
    }

    pub(super) fn set_preview_frame(&mut self, frame: Arc<Frame>) {
        self.preview.frame = Some(frame);
        self.preview.bitmap.take();
        self.dirty[PREVIEW] = true;
    }

    pub(super) fn take_preview_frame(&mut self) {
        let frame = self.worker.latest.lock().expect("playback mailbox").take();
        if let Some(frame) = frame
            && frame.generation == self.preview.generation
            && self.preview.playing
        {
            self.preview.position_ms = frame.position_ms;
            self.set_preview_frame(frame.frame);
        }
    }

    pub(super) fn stop_preview(&mut self) {
        self.take_preview_frame();
        self.preview.generation += 1;
        if let Some(cancel) = self.worker.playback_cancel.take() {
            cancel.cancel();
        }
        self.preview.playing = false;
        self.dirty[slot(Target::Play)] = true;
        self.events.push(Event::Name(Target::Play));
    }

    pub(super) fn start_preview(&mut self) {
        self.stop_preview();
        let cancel = CancelToken::default();
        let job = Job::Play {
            generation: self.preview.generation,
            position_ms: self.preview.position_ms,
            cancel: cancel.clone(),
        };
        if self.worker.jobs.send(job).is_err() {
            self.notify(
                "The recording session closed. Reopen the editor to preview.",
                true,
            );
            return;
        }
        self.worker.playback_cancel = Some(cancel);
        self.preview.playing = true;
        self.dirty[slot(Target::Play)] = true;
        self.events.push(Event::Name(Target::Play));
    }

    pub(super) fn layout_preview(&mut self) {
        let title = self.title_rect();
        let top = if self.warning().is_some() {
            let warning = self.warning_rect();
            warning.y + warning.h + self.t.n("s-5")
        } else {
            title.y + title.h + self.t.n("s-6")
        };
        // Playback decodes at up to 1280×720. Its texture resolution must not
        // change the accepted media's display size, scroll bounds or controls.
        let source = self.presented.as_ref().map(|p| &p.frame);
        let media_h = source.map_or(160., |f| {
            if self.preview.actual {
                f.height as f32
            } else {
                ((title.w - 2. - 2. * self.t.n("s-5")) * f.height as f32 / f.width as f32)
                    .min(self.height * 0.52)
            }
        });
        let height = (media_h + 2. * self.t.n("s-5")).clamp(160., self.height * 0.58);
        let height = height.min((self.rects[FOOTER].y - top - 46. - self.t.n("s-6")).max(1.));
        self.rects[PREVIEW] = Rect::new(title.x, top, title.w, 46. + height);
        let right = title.x + title.w - self.t.n("s-5");
        self.rects[slot(Target::Actual)] = Rect::new(right - 66., top + 7., 66., 32.);
        self.rects[slot(Target::Fit)] = Rect::new(right - 132., top + 7., 66., 32.);
        self.rects[slot(Target::Loop)] =
            Rect::new(right - 132. - self.t.n("s-4") - 132., top + 7., 132., 32.);
        self.clamp_preview_scroll();
        self.place_play();
        self.dirty[PREVIEW] = true;
        for target in [Target::Play, Target::Loop, Target::Fit, Target::Actual] {
            self.dirty[slot(target)] = true;
        }
    }

    pub(super) fn preview_view(&self) -> Rect {
        let r = self.rects[PREVIEW];
        Rect::new(r.x + 1., r.y + 46., r.w - 2., r.h - 47.)
    }

    pub(super) fn preview_media(&self) -> Rect {
        let r = self.preview_view();
        let pad = self.t.n("s-5");
        let Some(frame) = self.presented.as_ref().map(|p| &p.frame) else {
            return Rect::default();
        };
        let scale = if self.preview.actual {
            1.
        } else {
            ((r.w - 2. * pad) / frame.width as f32)
                .min((r.h - 2. * pad) / frame.height as f32)
                .max(0.)
        };
        let (w, h) = (frame.width as f32 * scale, frame.height as f32 * scale);
        Rect::new(
            r.x + ((r.w - w) / 2.).max(pad) - self.preview.scroll.0,
            r.y + ((r.h - h) / 2.).max(pad) - self.preview.scroll.1,
            w,
            h,
        )
    }

    pub(super) fn clamp_preview_scroll(&mut self) {
        let r = self.preview_view();
        let pad = 2. * self.t.n("s-5");
        let (w, h) = self
            .presented
            .as_ref()
            .map_or((0., 0.), |p| (p.frame.width as f32, p.frame.height as f32));
        self.preview.scroll = if self.preview.actual {
            (
                self.preview.scroll.0.clamp(0., (w + pad - r.w).max(0.)),
                self.preview.scroll.1.clamp(0., (h + pad - r.h).max(0.)),
            )
        } else {
            (0., 0.)
        };
    }

    pub(super) fn place_play(&mut self) {
        let media = self.preview_media();
        self.rects[slot(Target::Play)] = Rect::new(
            media.x + media.w / 2. - 28.,
            media.y + media.h / 2. - 28.,
            56.,
            56.,
        );
        self.dirty[slot(Target::Play)] = true;
    }

    pub fn play_clip(&self) -> Rect {
        let button = self.target_rect(Target::Play);
        let view = self.preview_view();
        let (x, y) = (button.x.max(view.x), button.y.max(view.y));
        Rect::new(
            x,
            y,
            ((button.x + button.w).min(view.x + view.w) - x).max(0.),
            ((button.y + button.h).min(view.y + view.h) - y).max(0.),
        )
    }

    pub(super) fn reveal_play(&mut self) {
        let view = self.preview_view();
        let button = self.target_rect(Target::Play);
        let clipped = self.play_clip();
        if clipped.w < button.w || clipped.h < button.h {
            self.preview.scroll.0 += button.x + button.w / 2. - view.x - view.w / 2.;
            self.preview.scroll.1 += button.y + button.h / 2. - view.y - view.h / 2.;
            self.clamp_preview_scroll();
            self.place_play();
            self.dirty[PREVIEW] = true;
        }
    }

    pub(super) fn draw_preview(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let r = self.rects[PREVIEW];
        let all = gfx::rect(0., 0., r.w, r.h);
        let t = &self.t;
        gfx::shadow(dc, all, t.n("r-xl"), t.shadow("shadow-sm"))?;
        gfx::fill_round(dc, all, t.n("r-xl"), t.color("surface-raised"))?;
        // The viewport and oversized media share the panel's rounded edge;
        // a rectangular stage fill must not paint over its bottom corners.
        let clip = unsafe {
            self.g
                .factory
                .CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                    rect: all,
                    radiusX: t.n("r-xl"),
                    radiusY: t.n("r-xl"),
                })?
        };
        let mut layer = D2D1_LAYER_PARAMETERS1 {
            contentBounds: all,
            geometricMask: std::mem::ManuallyDrop::new(Some(clip.cast()?)),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: 1.,
            ..Default::default()
        };
        unsafe { dc.PushLayer(&layer, None::<&ID2D1Layer>) };
        let result = self.draw_preview_content(dc);
        unsafe {
            dc.PopLayer();
            std::mem::ManuallyDrop::drop(&mut layer.geometricMask);
        }
        result?;
        gfx::border(dc, all, t.n("r-xl"), 1., t.color("border-subtle"))?;
        Ok(())
    }

    fn draw_preview_content(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let r = self.rects[PREVIEW];
        let t = &self.t;
        self.draw_text(
            dc,
            Style::Control,
            "Preview",
            Rect::new(t.n("s-5"), 0., 140., 46.),
            "text-muted",
            false,
        )?;
        let view = self.preview_view();
        let clip = gfx::rect(
            view.x - r.x,
            view.y - r.y,
            view.x + view.w - r.x,
            view.y + view.h - r.y,
        );
        gfx::fill(dc, clip, t.color("surface-sunken"))?;
        gfx::fill(
            dc,
            gfx::rect(1., 45., r.w - 1., 46.),
            t.color("border-subtle"),
        )?;
        let Some(frame) = &self.preview.frame else {
            return Ok(());
        };
        let mut cache = self.preview.bitmap.borrow_mut();
        unsafe {
            if cache.is_none() {
                *cache = Some(dc.CreateBitmap(
                    D2D_SIZE_U {
                        width: frame.width,
                        height: frame.height,
                    },
                    Some(frame.bgra.as_ptr().cast()),
                    frame.width * 4,
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
            let bitmap = cache.as_ref().expect("created bitmap");
            let media = self.preview_media();
            let media = gfx::rect(
                media.x - r.x,
                media.y - r.y,
                media.x + media.w - r.x,
                media.y + media.h - r.y,
            );
            let brush = dc.CreateBitmapBrush(
                bitmap,
                Some(&D2D1_BITMAP_BRUSH_PROPERTIES1 {
                    extendModeX: D2D1_EXTEND_MODE_CLAMP,
                    extendModeY: D2D1_EXTEND_MODE_CLAMP,
                    interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
                }),
                None,
            )?;
            brush.SetTransform(&Matrix3x2 {
                M11: (media.right - media.left) / frame.width as f32,
                M22: (media.bottom - media.top) / frame.height as f32,
                M31: media.left,
                M32: media.top,
                ..Matrix3x2::identity()
            });
            dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
            let result = gfx::fill_round(dc, media, t.n("r-md"), t.color("glass-strong-solid"));
            if result.is_ok() {
                dc.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: media,
                        radiusX: t.n("r-md"),
                        radiusY: t.n("r-md"),
                    },
                    &brush,
                );
            }
            dc.PopAxisAlignedClip();
            result?;
        }
        Ok(())
    }

    pub(super) fn draw_preview_control(
        &self,
        dc: &ID2D1DeviceContext,
        target: Target,
    ) -> Result<()> {
        let r = self.target_rect(target);
        let all = gfx::rect(0., 0., r.w, r.h);
        let t = &self.t;
        let hovered = self.hover == Some(target);
        if target == Target::Play {
            gfx::shadow(dc, all, 28., t.shadow("shadow-lg"))?;
            gfx::fill_round(
                dc,
                all,
                28.,
                t.color(if hovered {
                    "theme-accent-hover"
                } else {
                    "theme-accent"
                }),
            )?;
            gfx::border(dc, all, 28., 1., t.color("glass-border-strong"))?;
            gfx::icon(
                dc,
                &self.g.factory,
                if self.preview.playing {
                    "pause"
                } else {
                    "resume"
                },
                &[],
                (12., 12., 32.),
                2.,
                t.color("theme-accent-ink"),
            )?;
        } else {
            let selected = self.toggled(target) == Some(true);
            gfx::fill_round(
                dc,
                all,
                t.n("r-md"),
                t.color(if selected {
                    "surface-active"
                } else if hovered {
                    "surface-hover"
                } else {
                    "surface-raised"
                }),
            )?;
            if selected {
                gfx::border(dc, all, t.n("r-md"), 1., t.color("border"))?;
            }
            let text = self.g.text(t, Style::Control, target.label(), r.w)?;
            let icon_width = if target == Target::Loop { 20. } else { 0. };
            let x = (r.w - text.width - icon_width) / 2.;
            let color = t.color(if selected || hovered {
                "text"
            } else {
                "text-subtle"
            });
            if target == Target::Loop {
                gfx::icon(
                    dc,
                    &self.g.factory,
                    "loop",
                    &[],
                    (x, (r.h - 14.) / 2., 14.),
                    2.,
                    color,
                )?;
            }
            gfx::text(dc, &text, x + icon_width, (r.h - text.height) / 2., color)?;
        }
        if self.focus == Some(target) && self.focus_visible {
            gfx::ring(
                dc,
                all,
                if target == Target::Play {
                    28.
                } else {
                    t.n("r-md")
                },
                2.,
                4.,
                t.color("theme-accent"),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_resolution_playback_keeps_scrolled_media_filling_the_viewport() {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap() };
        let profile = std::env::temp_dir().join(format!(
            "captures-playback-size-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let options = crate::parse(
            ["captures", "--profile", profile.to_str().unwrap()]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap();
        let t = Tokens::variant("light", "mustard");
        let g = Gfx::new(&t, false).unwrap();
        let mut w = Recording::new(HWND::default(), &options, "missing", g.clone(), t).unwrap();
        // No decoder is needed: feed the same accepted-still/decoded-frame
        // boundary as the worker, with known pixels at different resolutions.
        let _ = w.worker.outcomes.recv().unwrap();
        let fixture = |width, height| {
            Arc::new(Frame::new(&RgbaImage::from_fn(width, height, |x, y| {
                image::Rgba(if x >= width * 7 / 8 && y >= height * 7 / 8 {
                    [37, 91, 153, 255]
                } else {
                    [173, 41, 88, 255]
                })
            })))
        };
        w.apply_outcome(Outcome::Opened(Ok(Presented {
            mime_type: "video/mp4".into(),
            dropped_frames: 0,
            export: captures_media::ExportSpec {
                format: captures_media::ExportFormat::Mp4,
                quality: captures_media::QualityPreset::Preserve,
                max_size_bytes: None,
                frames_per_second: None,
                gif_max_colors: None,
            },
            original: None,
            defaults: ui::SaveDefaults {
                directory: profile.to_string_lossy().into_owned(),
                stem: "Recording".into(),
            },
            frame: fixture(1920, 1080),
            duration_ms: 6000,
        })));
        (w.width, w.height) = (640., 480.);
        w.layout().unwrap();
        w.activate(Target::Actual);
        w.preview.scroll = (f32::MAX, f32::MAX);
        w.clamp_preview_scroll();
        w.place_play();

        let render_pixel = |w: &Recording| unsafe {
            let properties = |bitmap_options| D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.,
                dpiY: 96.,
                bitmapOptions: bitmap_options,
                ..Default::default()
            };
            let size = D2D_SIZE_U {
                width: 640,
                height: 480,
            };
            let target =
                g.dc.CreateBitmap(size, None, 0, &properties(D2D1_BITMAP_OPTIONS_TARGET))
                    .unwrap();
            g.dc.SetTarget(&target);
            g.dc.BeginDraw();
            w.paint(&g.dc).unwrap();
            g.dc.EndDraw(None, None).unwrap();
            g.dc.SetTarget(None);
            let read =
                g.dc.CreateBitmap(
                    size,
                    None,
                    0,
                    &properties(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW),
                )
                .unwrap();
            read.CopyFromBitmap(None, &target, None).unwrap();
            let mapped = read.Map(D2D1_MAP_OPTIONS_READ).unwrap();
            // Well inside the bottom-right media, away from rounded edges,
            // padding and Play. This must stay media, not the empty stage.
            let view = w.preview_view();
            let x = (view.x + view.w - 30.) as usize;
            let y = (view.y + view.h - 30.) as usize;
            let pixel =
                std::slice::from_raw_parts(mapped.bits.add(y * mapped.pitch as usize + x * 4), 4)
                    .to_vec();
            read.Unmap().unwrap();
            pixel
        };
        assert_eq!(render_pixel(&w), [153, 91, 37, 255]);
        w.preview.playing = true;
        *w.worker.latest.lock().unwrap() = Some(worker::PlaybackFrame {
            generation: w.preview.generation,
            position_ms: 100,
            frame: fixture(1280, 720),
        });
        w.take_preview_frame();
        assert_eq!(
            render_pixel(&w),
            [153, 91, 37, 255],
            "playback must not uncover the stage"
        );
        w.layout().unwrap();
        assert_eq!(
            render_pixel(&w),
            [153, 91, 37, 255],
            "relayout must preserve the same bottom-right source pixels"
        );
        drop(w);
        std::fs::remove_dir_all(profile).unwrap();
    }
}
