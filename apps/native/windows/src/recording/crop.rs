//! Source-pixel crop geometry shared by pointer, keyboard and UI Automation.
use captures_media::CropRect;

use super::*;

#[derive(Clone, Copy)]
pub(super) struct Drag {
    handle: CropDragHandle,
    initial: CropRect,
    start: (f32, f32),
    scale: (f64, f64),
}

pub(super) fn handle_label(handle: CropDragHandle) -> &'static str {
    use CropDragHandle::*;
    match handle {
        North => "Recording crop north handle",
        NorthEast => "Recording crop north-east handle",
        East => "Recording crop east handle",
        SouthEast => "Recording crop south-east handle",
        South => "Recording crop south handle",
        SouthWest => "Recording crop south-west handle",
        West => "Recording crop west handle",
        NorthWest => "Recording crop north-west handle",
        Move => "Move recording crop",
    }
}

fn vertical(handle: CropDragHandle) -> bool {
    matches!(handle, CropDragHandle::North | CropDragHandle::South)
}

fn edge(crop: CropRect, handle: CropDragHandle) -> f64 {
    use CropDragHandle::*;
    match handle {
        North => crop.y,
        South => crop.y + crop.height,
        West | NorthWest | SouthWest => crop.x,
        _ => crop.x + crop.width,
    }
    .into()
}

impl Recording {
    pub(super) fn crop_shown(&self) -> bool {
        self.output.values.crop_on && self.presented.as_ref().is_some_and(|p| p.full_source)
    }

    fn crop_rect(&self) -> Rect {
        let Some(p) = &self.presented else {
            return Rect::default();
        };
        let media = self.preview_media();
        let c = self.output.values.crop;
        let (sx, sy) = (media.w / p.width as f32, media.h / p.height as f32);
        Rect::new(
            media.x + c.x as f32 * sx,
            media.y + c.y as f32 * sy,
            c.width as f32 * sx,
            c.height as f32 * sy,
        )
    }

    pub(super) fn layout_crop(&mut self) {
        use CropDragHandle::*;
        let r = self.crop_rect();
        for (h, x, y) in [
            (NorthWest, r.x, r.y),
            (NorthEast, r.x + r.w, r.y),
            (SouthEast, r.x + r.w, r.y + r.h),
            (SouthWest, r.x, r.y + r.h),
            (North, r.x + r.w / 2., r.y),
            (East, r.x + r.w, r.y + r.h / 2.),
            (South, r.x + r.w / 2., r.y + r.h),
            (West, r.x, r.y + r.h / 2.),
        ] {
            let t = Target::CropHandle(h);
            self.rects[slot(t)] = Rect::new(x - 5.5, y - 5.5, 11., 11.);
            self.dirty[slot(t)] = true;
            self.events.push(Event::Range(t));
        }
    }

    pub(super) fn crop_hit(&self, x: f32, y: f32) -> Option<CropDragHandle> {
        if !self.crop_shown() || !self.page_visible_rect(self.preview_view()).contains(x, y) {
            return None;
        }
        use CropDragHandle::*;
        // Corners win when a tiny crop makes handle hit rectangles overlap.
        [
            NorthWest, NorthEast, SouthEast, SouthWest, North, East, South, West,
        ]
        .into_iter()
        .find(|&h| self.target_rect(Target::CropHandle(h)).contains(x, y))
        .or_else(|| self.crop_rect().contains(x, y).then_some(Move))
    }

    fn crop_after(
        &self,
        from: CropRect,
        handle: CropDragHandle,
        dx: f64,
        dy: f64,
    ) -> Option<CropRect> {
        let p = self.presented.as_ref()?;
        from.after_drag(p.width, p.height, handle, dx, dy, self.output.crop_locked)
    }

    fn move_crop(&mut self, from: CropRect, handle: CropDragHandle, dx: f64, dy: f64) {
        if let Some(crop) = self.crop_after(from, handle, dx, dy)
            && crop != self.output.values.crop
        {
            self.output.values.crop = crop;
            self.sync_output_fields();
            self.output_changed(Target::CropWidth);
            for target in [
                Target::CropX,
                Target::CropY,
                Target::CropHeight,
                Target::Resolution,
            ] {
                self.events.push(Event::OutputValue(target));
            }
        }
    }

    pub fn crop_range(&self, target: Target) -> Option<(f64, f64, f64)> {
        let Target::CropHandle(handle) = target else {
            return None;
        };
        if !self.crop_shown() {
            return None;
        }
        let c = self.output.values.crop;
        let p = self.presented.as_ref()?;
        let travel = f64::from(p.width.max(p.height)) * 2.;
        let delta = |v| if vertical(handle) { (0., v) } else { (v, 0.) };
        let bounds = |v| {
            let (x, y) = delta(v);
            self.crop_after(c, handle, x, y).map(|c| edge(c, handle))
        };
        Some((edge(c, handle), bounds(-travel)?, bounds(travel)?))
    }

    pub fn set_crop_range(&mut self, target: Target, value: f64) {
        if !self.enabled(target) || !value.is_finite() {
            return;
        }
        let Target::CropHandle(handle) = target else {
            return;
        };
        let Some((current, min, max)) = self.crop_range(target) else {
            return;
        };
        let delta = value.round().clamp(min, max) - current;
        let (dx, dy) = if vertical(handle) {
            (0., delta)
        } else {
            (delta, 0.)
        };
        self.move_crop(self.output.values.crop, handle, dx, dy);
    }

    pub(super) fn crop_key(&mut self, key: VIRTUAL_KEY, shift: bool) -> bool {
        if key == VK_ESCAPE && self.crop_drag.is_some() {
            self.end_crop_drag();
            unsafe {
                let _ = ReleaseCapture();
            }
            return true;
        }
        let Some(target @ Target::CropHandle(handle)) = self.focus else {
            return false;
        };
        if !self.enabled(target) {
            return false;
        }
        let step = if shift { 10. } else { 1. };
        let (dx, dy) = match key {
            VK_LEFT => (-step, 0.),
            VK_RIGHT => (step, 0.),
            VK_UP => (0., -step),
            VK_DOWN => (0., step),
            VK_HOME | VK_END => {
                if let Some((_, min, max)) = self.crop_range(target) {
                    self.set_crop_range(target, if key == VK_HOME { min } else { max });
                }
                return true;
            }
            _ => return false,
        };
        self.move_crop(self.output.values.crop, handle, dx, dy);
        self.reveal_crop_handle(target);
        true
    }

    pub(super) fn end_crop_drag(&mut self) {
        self.crop_drag = None;
        self.resume_timeline();
    }

    pub(super) fn crop_pointer(&mut self, msg: u32, x: f32, y: f32) -> bool {
        if msg == WM_LBUTTONDOWN {
            if !self.enabled(Target::Crop) {
                return false;
            }
            let Some(handle) = self.crop_hit(x, y) else {
                return false;
            };
            self.set_focus(
                (handle != CropDragHandle::Move).then_some(Target::CropHandle(handle)),
                false,
            );
            let media = self.preview_media();
            let p = self.presented.as_ref().expect("crop source");
            self.crop_drag = Some(Drag {
                handle,
                initial: self.output.values.crop,
                start: (x, y),
                scale: (
                    f64::from(p.width) / f64::from(media.w),
                    f64::from(p.height) / f64::from(media.h),
                ),
            });
            self.timeline.resume |= self.preview.playing;
            self.stop_preview();
            unsafe {
                let _ = SetFocus(Some(self.hwnd));
                SetCapture(self.hwnd);
            }
            return true;
        }
        let Some(drag) = self.crop_drag else {
            return false;
        };
        if msg == WM_MOUSEMOVE || msg == WM_LBUTTONUP {
            self.move_crop(
                drag.initial,
                drag.handle,
                f64::from(x - drag.start.0) * drag.scale.0,
                f64::from(y - drag.start.1) * drag.scale.1,
            );
        }
        if msg == WM_LBUTTONUP {
            self.end_crop_drag();
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        true
    }

    pub(super) fn reveal_crop_handle(&mut self, target: Target) {
        if !matches!(target, Target::CropHandle(_)) {
            return;
        }
        let view = self.preview_view();
        let r = self.target_rect(target);
        let delta = |a: f32, b: f32, n: f32, size: f32| {
            if a < b + 4. {
                a - b - 4.
            } else {
                (a + n + 4. - b - size).max(0.)
            }
        };
        self.preview.scroll.0 += delta(r.x, view.x, r.w, view.w);
        self.preview.scroll.1 += delta(r.y, view.y, r.h, view.h);
        self.clamp_preview_scroll();
        self.place_play();
        self.dirty[PREVIEW] = true;
    }

    pub(super) fn draw_crop(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        if !self.crop_shown() {
            return Ok(());
        }
        let origin = self.rects[PREVIEW];
        let media = self.preview_media();
        let c = self.crop_rect();
        let t = &self.t;
        let rect = |r: Rect| {
            gfx::rect(
                r.x - origin.x,
                r.y - origin.y,
                r.x + r.w - origin.x,
                r.y + r.h - origin.y,
            )
        };
        let view = self.preview_view();
        unsafe {
            dc.PushAxisAlignedClip(&rect(view), D2D1_ANTIALIAS_MODE_ALIASED);
        }
        let result = (|| {
            for dim in [
                Rect::new(media.x, media.y, media.w, c.y - media.y),
                Rect::new(media.x, c.y, c.x - media.x, c.h),
                Rect::new(c.x + c.w, c.y, media.x + media.w - c.x - c.w, c.h),
                Rect::new(media.x, c.y + c.h, media.w, media.y + media.h - c.y - c.h),
            ] {
                gfx::fill(dc, rect(dim), t.color_alpha("glass-strong-solid", 0.62))?;
            }
            gfx::border(
                dc,
                rect(c),
                0.,
                3.5,
                t.color_alpha("glass-strong-solid", 0.6),
            )?;
            gfx::border(dc, rect(c), 0., 1.5, t.color("theme-accent"))?;
            let crop = self.output.values.crop;
            let text = self.g.text(
                t,
                Style::Tiny,
                &format!("{} × {}", crop.width, crop.height),
                200.,
            )?;
            let pad = t.n("s-3");
            let label = Rect::new(
                c.x + c.w / 2. - text.width / 2. - pad,
                c.y + pad,
                text.width + 2. * pad,
                text.height + 6.,
            );
            gfx::fill_round(dc, rect(label), t.n("r-xs"), t.color("theme-accent"))?;
            gfx::text(
                dc,
                &text,
                label.x - origin.x + pad,
                label.y - origin.y + 3.,
                t.color("theme-accent-ink"),
            )
        })();
        unsafe {
            dc.PopAxisAlignedClip();
        }
        result
    }

    pub(super) fn draw_crop_handle(&self, dc: &ID2D1DeviceContext, target: Target) -> Result<()> {
        let t = &self.t;
        let r = gfx::rect(0., 0., 11., 11.);
        gfx::fill_round(dc, r, 3., t.color("theme-accent"))?;
        gfx::border(dc, r, 3., 2., t.color("theme-accent-ink"))?;
        if self.focus == Some(target) && self.focus_visible {
            gfx::ring(dc, r, 3., 2., 4., t.color("theme-accent"))?;
        }
        Ok(())
    }
}
