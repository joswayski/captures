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
    pub fn bitmap(&self, dc: &ID2D1DeviceContext) -> Result<ID2D1Bitmap1> {
        unsafe {
            dc.CreateBitmap(
                D2D_SIZE_U {
                    width: self.width,
                    height: self.height,
                },
                Some(self.bgra.as_ptr().cast()),
                self.width * 4,
                &D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.,
                    dpiY: 96.,
                    ..Default::default()
                },
            )
        }
    }

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
            Target::System => Some(!self.output.values.audio.mute_system_audio),
            Target::Microphone => Some(!self.output.values.audio.mute_microphone),
            Target::Mono => Some(self.output.values.audio.mono_output),
            Target::Crop => Some(self.output.values.crop_on),
            Target::CropLock => Some(self.output.crop_locked),
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
            self.dirty[TIMELINE] = true;
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
        self.layout_timeline();
        self.layout_output();
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
        self.layout_crop();
    }

    pub fn play_clip(&self) -> Rect {
        self.preview_control_clip(Target::Play)
    }

    pub(super) fn preview_control_clip(&self, target: Target) -> Rect {
        let button = self.target_rect(target);
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
                *cache = Some(frame.bitmap(dc)?);
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
        self.draw_crop(dc)
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

    fn frame(width: u32, height: u32) -> Arc<Frame> {
        Arc::new(Frame::new(&RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba(if x >= width * 7 / 8 && y >= height * 7 / 8 {
                [37, 91, 153, 255]
            } else {
                [173, 41, 88, 255]
            })
        })))
    }

    fn fixture() -> (Recording, PathBuf) {
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
            frame: frame(1920, 1080),
            duration_ms: 6000,
            edit: captures_media::EditSpec::default(),
            position_ms: 0,
            width: 1920,
            height: 1080,
            output: output::Values::new(1920, 1080, &Default::default()),
            full_source: false,
        })));
        (w, profile)
    }

    #[test]
    fn lower_resolution_playback_keeps_scrolled_media_filling_the_viewport() {
        let (mut w, profile) = fixture();
        let g = w.g.clone();
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
            // Well inside the visible bottom-right media, away from rounded
            // edges, padding, Play and the fixed footer. At minimum size the
            // bottom of the full media view is clipped behind that footer.
            let view = w.page_visible_rect(w.preview_view());
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
            frame: frame(1280, 720),
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

    #[test]
    fn play_restarts_within_the_final_ten_milliseconds_on_the_first_press() {
        let (mut w, profile) = fixture();
        w.timeline.start = 1100;
        for (end, position, expected) in [
            (6000, 5999, 1100),
            (6000, 5990, 1100),
            (6000, 5989, 5989),
            (4200, 4190, 1100),
            (4200, 4189, 4189),
            (1105, 1104, 1100),
        ] {
            w.timeline.end = end;
            w.preview.position_ms = position;
            w.activate(Target::Play);
            assert!(w.preview.playing);
            assert_eq!(
                w.preview.position_ms, expected,
                "end={end}, position={position}"
            );
            w.activate(Target::Play);
            assert!(!w.preview.playing);
        }
        drop(w);
        std::fs::remove_dir_all(profile).unwrap();
    }

    #[test]
    fn looping_preview_restarts_an_empty_tail_but_not_an_empty_trim() {
        for (looping, position, restarts) in [
            (true, 5999, true),
            (false, 5999, false),
            (true, 1100, false),
        ] {
            let (mut w, profile) = fixture();
            w.presented.as_mut().unwrap().mime_type = "image/gif".into();
            w.timeline.start = 1100;
            w.preview.position_ms = position;
            w.preview.playing = true;
            w.worker.looping.store(looping, Ordering::Relaxed);
            w.apply_outcome(Outcome::PlaybackEnded(w.preview.generation, Ok(())));
            assert!(
                !w.preview.playing,
                "wait for the starting frame before replay"
            );
            let mut accepted = w.presented.take().unwrap();
            accepted.edit.trim_start_ms = 1100;
            accepted.position_ms = 1100;
            w.apply_outcome(Outcome::Preview(
                w.timeline.generation,
                Box::new(accepted),
                None,
            ));
            assert_eq!(w.preview.position_ms, 1100);
            assert_eq!(
                w.preview.playing, restarts,
                "looping={looping}, position={position}"
            );
            assert!(!w.error);
            drop(w);
            std::fs::remove_dir_all(profile).unwrap();
        }
    }

    #[test]
    fn timeline_keeps_source_time_and_only_accepts_the_latest_preview() {
        let (mut w, profile) = fixture();
        (w.width, w.height) = (640., 480.);
        w.layout().unwrap();
        let footer = w.target_rect(Target::Save);
        // Keyboard focus must reveal the whole handle above the fixed footer.
        w.set_focus(Some(Target::TrimStart), true);
        let handle = w.visible_target_rect(Target::TrimStart);
        assert_eq!(handle.h, 82.);
        assert!(handle.y >= 0. && handle.y + handle.h < footer.y);
        assert_eq!(w.target_rect(Target::Save).y, footer.y);
        w.key(VK_RIGHT);
        assert_eq!(w.trim_range(Target::TrimStart), Some((1, 0, 5999)));
        w.key(VK_PRIOR);
        assert_eq!(w.trim_range(Target::TrimStart), Some((1001, 0, 5999)));
        w.set_trim(Target::TrimEnd, 4200.);
        assert_eq!(w.trim_range(Target::TrimEnd), Some((4200, 1002, 6000)));

        // Full-duration trim endpoints are valid, but preview seeks must
        // remain inside the source's final presentation interval.
        w.set_trim(Target::TrimEnd, 6000.);
        assert_eq!(w.trim_range(Target::TrimEnd), Some((6000, 1002, 6000)));
        assert_eq!(w.preview.position_ms, 5999);
        w.set_focus(Some(Target::TrimEnd), true);
        w.set_trim(Target::TrimEnd, 5999.);
        w.key(VK_RIGHT);
        assert_eq!((w.timeline.end, w.preview.position_ms), (6000, 5999));
        w.set_trim(Target::TrimEnd, 5000.);
        w.key(VK_PRIOR);
        assert_eq!((w.timeline.end, w.preview.position_ms), (6000, 5999));
        w.set_trim(Target::TrimEnd, 4200.);
        let track = w.timeline_track();
        assert!(w.timeline_pointer(WM_LBUTTONDOWN, track.x + track.w - 1., track.y + 20.));
        assert!(w.timeline_pointer(WM_MOUSEMOVE, track.x + track.w, track.y + 20.));
        assert_eq!(w.preview.position_ms, 5999);
        assert_eq!(w.timeline.end, 4200, "scrubbing must not alter the trim");
        w.end_timeline_drag();

        // Scrubbing is source-relative, not restricted to the selected range.
        let track = w.timeline_track();
        assert!(w.timeline_pointer(WM_LBUTTONDOWN, track.x + track.w / 12., track.y + 20.));
        assert_eq!(w.preview.position_ms, 500);
        assert!(!w.enabled(Target::Save));
        assert!(!w.enabled(Target::Play));
        w.end_timeline_drag();
        let generation = w.timeline.generation;

        let mut stale = w.presented.take().unwrap();
        stale.edit.trim_start_ms = 100;
        stale.edit.trim_end_ms = Some(5000);
        w.apply_outcome(Outcome::Preview(generation - 1, Box::new(stale), None));
        assert_eq!((w.timeline.start, w.timeline.end), (1001, 4200));
        assert!(w.timeline.pending);

        // A failed latest decode must restore the worker's accepted edit,
        // including an earlier success whose stale UI outcome was ignored.
        let (mut source, source_profile) = fixture();
        let mut accepted = source.presented.take().unwrap();
        accepted.edit.trim_start_ms = 100;
        accepted.edit.trim_end_ms = Some(5000);
        accepted.position_ms = 800;
        w.apply_outcome(Outcome::Preview(
            generation,
            Box::new(accepted),
            Some("decode failed".into()),
        ));
        assert_eq!(
            (w.timeline.start, w.timeline.end, w.preview.position_ms),
            (100, 5000, 800)
        );
        assert!(w.enabled(Target::Save) && w.error);
        w.set_trim(Target::TrimStart, 6000.);
        assert_eq!(w.trim_range(Target::TrimStart), Some((4999, 0, 4999)));
        w.set_trim(Target::TrimEnd, 0.);
        assert_eq!(w.trim_range(Target::TrimEnd), Some((5000, 5000, 6000)));

        w.presented.as_mut().unwrap().duration_ms = 75_000;
        w.set_trim(Target::TrimEnd, 74_990.);
        w.key(VK_RIGHT);
        assert_eq!((w.timeline.end, w.preview.position_ms), (75_000, 74_999));
        w.set_focus(Some(Target::TrimStart), true);
        w.set_trim(Target::TrimStart, 1000.);
        w.key(VK_RIGHT);
        assert_eq!(w.timeline.start, 1010);
        let accepted = w.presented.take().unwrap();
        w.apply_outcome(Outcome::Preview(
            w.timeline.generation,
            Box::new(accepted),
            None,
        ));
        assert!(!w.error && w.status.is_empty());
        drop(w);
        drop(source);
        std::fs::remove_dir_all(profile).unwrap();
        std::fs::remove_dir_all(source_profile).unwrap();
    }

    #[test]
    fn crop_controls_share_source_coordinates_and_keep_handles_visible() {
        use captures_media::{CropDragHandle::*, CropRect};
        let (mut w, profile) = fixture();
        (w.width, w.height) = (640., 480.);
        w.layout().unwrap();
        assert!(!w.enabled(Target::CropWidth));
        w.activate(Target::Crop);
        w.presented.as_mut().unwrap().full_source = true;
        w.set_output_number(Target::CropWidth, "640");
        w.set_output_number(Target::CropX, "17");
        w.set_output_number(Target::CropY, "29");
        assert_eq!(w.output_value(Target::CropHeight), "360");
        w.set_crop_range(Target::CropHandle(East), 625.);
        assert_eq!(
            w.output.values.crop,
            CropRect {
                x: 17,
                y: 38,
                width: 608,
                height: 342
            }
        );
        assert_eq!(w.crop_range(Target::CropHandle(East)).unwrap().0, 625.);
        w.activate(Target::CropLock);
        w.set_focus(Some(Target::CropHandle(NorthWest)), true);
        assert!(w.crop_key(VK_DOWN, true));
        assert_eq!(
            w.output.values.crop,
            CropRect {
                x: 17,
                y: 48,
                width: 608,
                height: 332
            }
        );
        w.set_output_number(Target::CropWidth, "640");
        w.set_output_number(Target::CropHeight, "360");
        w.set_output_number(Target::CropX, "100");
        w.set_output_number(Target::CropY, "200");
        w.scroll_page(-f32::MAX);
        let media = w.preview_media();
        let (sx, sy) = (media.w / 1920., media.h / 1080.);
        let (x, y) = (media.x + 200. * sx, media.y + 290. * sy);
        assert!(w.crop_pointer(WM_LBUTTONDOWN, x, y));
        assert!(w.crop_pointer(WM_MOUSEMOVE, x + 37. * sx, y - 23. * sy));
        assert_eq!(w.output.values.crop.x, 137);
        assert_eq!(w.output.values.crop.y, 177);
        assert!(w.crop_pointer(WM_LBUTTONUP, x + 100. * sx, y - 23. * sy));
        assert_eq!(
            w.output.values.crop,
            CropRect {
                x: 200,
                y: 177,
                width: 640,
                height: 360
            }
        );
        assert!(w.crop_drag.is_none());
        assert!(!w.enabled(Target::Save));
        w.set_output_number(Target::CropX, "1280");
        w.set_output_number(Target::CropY, "720");
        w.activate(Target::Actual);
        w.set_focus(Some(Target::CropHandle(SouthEast)), true);
        let target = w.target_rect(Target::CropHandle(SouthEast));
        let visible = w.visible_target_rect(Target::CropHandle(SouthEast));
        assert_eq!((visible.w, visible.h), (11., 11.));
        assert!(target.y + target.h + 4. <= w.rects[FOOTER].y);
        w.activate(Target::Crop);
        assert!(!w.shown(Target::CropHandle(SouthEast)));
        assert!(!w.enabled(Target::CropWidth));
        drop(w);
        std::fs::remove_dir_all(profile).unwrap();
    }

    #[test]
    fn output_controls_keep_drafts_ranges_and_accepted_settings_together() {
        use captures_app::recording_editor_ui::ResolutionChoice;
        let (mut w, profile) = fixture();
        (w.width, w.height) = (640., 480.);
        let p = w.presented.as_mut().unwrap();
        p.edit.audio.source_has_system_audio = true;
        p.edit.audio.source_has_microphone_audio = true;
        w.layout().unwrap();
        w.set_output_menu(Target::Resolution, true);
        w.key(VK_END);
        w.key(VK_ESCAPE);
        assert!(
            !w.shown(Target::Width),
            "highlighting then Escape does not choose"
        );
        w.set_output_menu(Target::Resolution, true);
        w.key(VK_END);
        w.key(VK_RETURN);
        assert!(w.shown(Target::Width));
        w.set_focus(Some(Target::Width), true);
        assert_eq!(
            w.visible_target_rect(Target::Width).h,
            w.target_rect(Target::Width).h
        );
        w.set_output_number(Target::Width, "1");
        assert_eq!(
            w.output.values.custom.0, 1920,
            "incomplete text must not resize"
        );
        w.set_output_number(Target::Width, "641");
        w.set_output_number(Target::Height, "479");
        let p = w.presented.as_ref().unwrap();
        let (edit, _) = w
            .output
            .values
            .specs(p.width, p.height, p.edit.clone(), p.export.clone());
        assert_eq!(
            (edit.output_width, edit.output_height),
            (Some(640), Some(478))
        );
        assert!(
            !w.enabled(Target::Save),
            "pending settings cannot save the previous edit"
        );
        w.set_output_number(Target::Width, "");
        w.key(VK_UP);
        assert_eq!(
            w.output_value(Target::Width),
            "642",
            "empty draft steps from the current value"
        );
        w.set_output_number(Target::Width, "0");
        w.set_focus(Some(Target::Height), true);
        assert_eq!(
            w.output_value(Target::Width),
            "2",
            "blur clamps to the minimum"
        );

        w.set_volume(Target::SystemVolume, 37.);
        w.set_volume(Target::MicrophoneVolume, 161.);
        w.activate(Target::System);
        assert_eq!(w.toggled(Target::System), Some(false));
        assert!(!w.enabled(Target::SystemVolume));
        w.set_volume(Target::SystemVolume, 80.);
        assert_eq!(w.volume_range(Target::SystemVolume).unwrap().0, 37.);
        w.activate(Target::System);
        w.activate(Target::Mono);
        let p = w.presented.as_ref().unwrap();
        let (edit, _) = w
            .output
            .values
            .specs(p.width, p.height, p.edit.clone(), p.export.clone());
        assert_eq!(
            (edit.audio.system_volume, edit.audio.microphone_volume),
            (0.37, 1.61)
        );
        assert!(edit.audio.mono_output && edit.audio.source_has_microphone_audio);
        assert!(!edit.audio.mute_system_audio);
        w.set_focus(Some(Target::MicrophoneVolume), true);
        w.key(VK_HOME);
        w.key(VK_PRIOR);
        assert_eq!(
            w.volume_range(Target::MicrophoneVolume),
            Some((20., 0., 200.))
        );

        let mut accepted = w.presented.take().unwrap();
        accepted.output.resolution = ResolutionChoice::P720;
        accepted.edit.output_width = Some(1280);
        accepted.edit.output_height = Some(720);
        w.apply_outcome(Outcome::Preview(
            w.timeline.generation,
            Box::new(accepted),
            Some("decode failed".into()),
        ));
        assert_eq!(w.output_value(Target::Resolution), "720p maximum");
        assert!(!w.shown(Target::Width));
        assert_eq!(w.volume_range(Target::SystemVolume).unwrap().0, 100.);
        assert!(w.enabled(Target::Save) && w.error);
        drop(w);
        std::fs::remove_dir_all(profile).unwrap();
    }
}
