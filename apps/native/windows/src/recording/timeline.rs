//! Source-relative trim/scrub controls. Drag state stays on the UI thread;
//! the worker coalesces decoding and publishes accepted edits atomically.
use captures_app::recording_timeline::{
    TimelineTrimDrag, TimelineTrimEdge, timeline_time_at_client_x,
};

use super::*;

#[derive(Default)]
pub(super) struct Timeline {
    pub start: u64,
    pub end: u64,
    pub generation: u64,
    pub pending: bool,
    pub resume: bool,
    pub drag: Option<(Target, TimelineTrimDrag)>,
    pub scrubbing: bool,
    pub strip: Option<ID2D1Bitmap1>,
}

impl Recording {
    pub fn trim_range(&self, target: Target) -> Option<(u64, u64, u64)> {
        let duration = self.presented.as_ref()?.duration_ms.max(1);
        match target {
            Target::TrimStart => {
                Some((self.timeline.start, 0, self.timeline.end.saturating_sub(1)))
            }
            Target::TrimEnd => Some((self.timeline.end, self.timeline.start + 1, duration)),
            _ => None,
        }
    }

    pub(super) fn timeline_track(&self) -> Rect {
        let r = self.rects[TIMELINE];
        let pad = self.t.n("s-6");
        Rect::new(
            r.x + pad + 1.,
            r.y + self.t.n("s-5") + 20. + self.t.n("s-4"),
            r.w - 2. * pad - 2.,
            ui::TIMELINE_TRACK_HEIGHT,
        )
    }

    pub(super) fn layout_timeline(&mut self) {
        let preview = self.rects[PREVIEW];
        self.rects[TIMELINE] = Rect::new(
            preview.x,
            preview.y + preview.h + self.t.n("s-5"),
            preview.w,
            self.timeline_height(),
        );
        let track = self.timeline_track();
        let inset = self.t.n("s-2");
        let duration = self.presented.as_ref().map_or(1, |p| p.duration_ms.max(1)) as f64;
        for (target, time) in [
            (Target::TrimStart, self.timeline.start),
            (Target::TrimEnd, self.timeline.end),
        ] {
            let x = track.x + inset + (track.w - 2. * inset) * (time as f64 / duration) as f32;
            self.rects[slot(target)] = Rect::new(x - inset, track.y - 3., 2. * inset, track.h + 6.);
            self.dirty[slot(target)] = true;
        }
        self.dirty[TIMELINE] = true;
    }

    pub(super) fn timeline_height(&self) -> f32 {
        self.t.n("s-5") + 20. + self.t.n("s-4") + ui::TIMELINE_TRACK_HEIGHT + self.t.n("s-6") + 2.
    }

    pub(super) fn load_timeline(&mut self) {
        self.timeline.strip = None;
        let cancel = CancelToken::default();
        self.worker.thumbnail_cancel = Some(cancel.clone());
        let _ = self.worker.jobs.send(Job::Thumbnails(cancel));
    }

    pub fn set_trim(&mut self, target: Target, value: f64) {
        let Some((_, min, max)) = self.trim_range(target) else {
            return;
        };
        if !self.enabled(target) || !value.is_finite() {
            return;
        }
        let value = value.round().clamp(min as f64, max as f64) as u64;
        if target == Target::TrimStart {
            self.timeline.start = value;
        } else {
            self.timeline.end = value;
        }
        self.saved = None;
        self.saved_fingerprint = None;
        self.seek_timeline(value);
        self.events.push(Event::Range(target));
        self.layout_timeline();
    }

    pub(super) fn seek_timeline(&mut self, position_ms: u64) {
        let Some(p) = self.presented.as_ref() else {
            return;
        };
        let mut edit = p.edit.clone();
        edit.trim_start_ms = self.timeline.start;
        edit.trim_end_ms = Some(self.timeline.end);
        let (edit, export) = self
            .output
            .values
            .specs(p.width, p.height, edit, p.export.clone());
        // Trim end may equal duration; a preview frame must be inside the
        // source's final presentation interval, as in the AppKit host.
        let position_ms = position_ms.min(p.duration_ms.saturating_sub(1));
        self.timeline.resume |= self.preview.playing;
        self.stop_preview();
        self.preview.position_ms = position_ms;
        self.timeline.generation += 1;
        self.timeline.pending = true;
        self.output.due = None;
        if !self.hwnd.is_invalid() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), OUTPUT_TIMER);
            }
        }
        self.worker.preview(worker::PreviewRequest {
            generation: self.timeline.generation,
            edit,
            position_ms,
            export,
            output: self.output.values.clone(),
        });
        self.dirty[TIMELINE] = true;
        self.refresh_footer();
    }

    pub(super) fn timeline_pointer(&mut self, msg: u32, x: f32, y: f32) -> bool {
        if !self.enabled(Target::TrimStart) {
            return false;
        }
        let track = self.timeline_track();
        let duration = self.presented.as_ref().map_or(1, |p| p.duration_ms.max(1));
        if msg == WM_LBUTTONDOWN {
            if y < 0. || y >= self.rects[FOOTER].y {
                return false;
            }
            let target = [Target::TrimEnd, Target::TrimStart]
                .into_iter()
                .find(|&t| self.target_rect(t).contains(x, y));
            if let Some(target) = target {
                let edge = if target == Target::TrimStart {
                    TimelineTrimEdge::Start
                } else {
                    TimelineTrimEdge::End
                };
                self.timeline.drag = TimelineTrimDrag::begin(
                    edge,
                    x.into(),
                    self.timeline.start as f64,
                    self.timeline.end as f64,
                    duration as f64,
                )
                .map(|drag| (target, drag));
                self.set_focus(Some(target), false);
                self.seek_timeline(if target == Target::TrimStart {
                    self.timeline.start
                } else {
                    self.timeline.end
                });
            } else if track.contains(x, y) {
                self.timeline.scrubbing = true;
            } else {
                return false;
            }
            unsafe {
                let _ = SetFocus(Some(self.hwnd));
                SetCapture(self.hwnd);
            }
        }
        if !self.timeline.scrubbing && self.timeline.drag.is_none() {
            return false;
        }
        if msg == WM_MOUSEMOVE || msg == WM_LBUTTONDOWN {
            if let Some((target, drag)) = self.timeline.drag {
                if let Some(update) = drag.update(x.into(), track.x.into(), track.w.into()) {
                    self.timeline.drag = Some((target, update.drag));
                    if update.drag.dragging {
                        self.set_trim(target, update.time_ms);
                    }
                }
            } else if let Some(time) =
                timeline_time_at_client_x(x.into(), track.x.into(), track.w.into(), duration as f64)
            {
                self.seek_timeline(time.round() as u64);
            }
        }
        if msg == WM_LBUTTONUP {
            self.end_timeline_drag();
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        true
    }

    pub(super) fn end_timeline_drag(&mut self) {
        self.timeline.drag = None;
        self.timeline.scrubbing = false;
        self.resume_timeline();
    }

    pub(super) fn resume_timeline(&mut self) {
        if self.timeline.resume
            && !self.timeline.pending
            && self.timeline.drag.is_none()
            && !self.timeline.scrubbing
            && !self.closing
            && !self.saving
        {
            self.timeline.resume = false;
            self.start_preview();
        }
    }

    pub(super) fn draw_timeline(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let r = self.rects[TIMELINE];
        let t = &self.t;
        let local = gfx::rect(0., 0., r.w, r.h);
        gfx::shadow(dc, local, t.n("r-xl"), t.shadow("shadow-sm"))?;
        gfx::fill_round(dc, local, t.n("r-xl"), t.color("surface-raised"))?;
        gfx::border(dc, local, t.n("r-xl"), 1., t.color("border-subtle"))?;
        let duration = self.presented.as_ref().map_or(1, |p| p.duration_ms.max(1));
        let summary = ui::trim_summary(self.timeline.start, self.timeline.end, duration);
        let line = Rect::new(t.n("s-6"), t.n("s-5"), r.w - 2. * t.n("s-6"), 20.);
        self.draw_text(dc, Style::Strong, &summary.range, line, "text", false)?;
        self.draw_text(
            dc,
            Style::Option,
            &summary.selected,
            line,
            "text-subtle",
            true,
        )?;
        let track = self.timeline_track();
        let (x, y) = (track.x - r.x, track.y - r.y);
        let bounds = gfx::rect(x, y, x + track.w, y + track.h);
        gfx::fill_round(dc, bounds, t.n("r-md"), t.color("surface-sunken"))?;
        let inset = t.n("s-2");
        if let Some(strip) = &self.timeline.strip {
            unsafe {
                let brush = dc.CreateBitmapBrush(
                    strip,
                    Some(&D2D1_BITMAP_BRUSH_PROPERTIES1 {
                        extendModeX: D2D1_EXTEND_MODE_CLAMP,
                        extendModeY: D2D1_EXTEND_MODE_CLAMP,
                        interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
                    }),
                    None,
                )?;
                let size = strip.GetSize();
                brush.SetTransform(&Matrix3x2 {
                    M11: (track.w - 2. * inset) / size.width,
                    M22: (track.h - 2. * inset) / size.height,
                    M31: x + inset,
                    M32: y + inset,
                    ..Matrix3x2::identity()
                });
                dc.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: gfx::rect(
                            x + inset,
                            y + inset,
                            x + track.w - inset,
                            y + track.h - inset,
                        ),
                        radiusX: t.n("r-xs"),
                        radiusY: t.n("r-xs"),
                    },
                    &brush,
                );
            }
        }
        let dim = t.color_alpha(
            "surface-canvas",
            if t.appearance == "light" { 0.76 } else { 0.66 },
        );
        for (left, right) in [
            (0., self.timeline.start as f32 / duration as f32),
            (self.timeline.end as f32 / duration as f32, 1.),
        ] {
            unsafe {
                dc.PushAxisAlignedClip(
                    &gfx::rect(x + track.w * left, y, x + track.w * right, y + track.h),
                    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                );
            }
            let result = gfx::fill_round(dc, bounds, t.n("r-md"), dim);
            unsafe {
                dc.PopAxisAlignedClip();
            }
            result?;
        }
        gfx::border(dc, bounds, t.n("r-md"), 1., t.color("border"))?;
        let head = x + track.w * self.preview.position_ms as f32 / duration as f32;
        gfx::fill(
            dc,
            gfx::rect(head - 1., y - 3., head + 1., y + track.h + 3.),
            t.color("text"),
        )?;
        gfx::fill_round(
            dc,
            gfx::rect(head - 5., y - 6., head + 5., y + 2.),
            2.,
            t.color("text"),
        )?;
        for target in [Target::TrimStart, Target::TrimEnd] {
            if self.hover != Some(target)
                && !(self.focus == Some(target) && self.focus_visible)
                && !self
                    .timeline
                    .drag
                    .is_some_and(|(dragged, _)| dragged == target)
            {
                continue;
            }
            let handle = self.target_rect(target);
            let time = if target == Target::TrimStart {
                self.timeline.start
            } else {
                self.timeline.end
            };
            let text = self.g.text(
                t,
                Style::Tiny,
                &ui::format_editor_time(time, duration),
                200.,
            )?;
            let width = text.width + 2. * t.n("s-3");
            let left = handle.x - r.x
                + if target == Target::TrimEnd {
                    handle.w - width
                } else {
                    0.
                };
            let top = handle.y - r.y - 5. - text.height - 6.;
            gfx::fill_round(
                dc,
                gfx::rect(left, top, left + width, top + text.height + 6.),
                t.n("r-xs"),
                t.color("theme-accent"),
            )?;
            gfx::text(
                dc,
                &text,
                left + t.n("s-3"),
                top + 3.,
                t.color("theme-accent-ink"),
            )?;
        }
        Ok(())
    }

    pub(super) fn draw_trim_handle(&self, dc: &ID2D1DeviceContext, target: Target) -> Result<()> {
        let r = self.target_rect(target);
        let t = &self.t;
        let local = gfx::rect(0., 0., r.w, r.h);
        gfx::shadow(dc, local, t.n("r-xs"), t.shadow("shadow-sm"))?;
        gfx::fill_round(dc, local, t.n("r-xs"), t.color("theme-accent"))?;
        for x in [t.n("s-1"), r.w - t.n("s-1") - 1.] {
            gfx::fill(
                dc,
                gfx::rect(x, 34., x + 1., 48.),
                t.color_alpha("theme-accent-ink", 0.45),
            )?;
        }
        if self.focus == Some(target) && self.focus_visible {
            gfx::ring(dc, local, t.n("r-xs"), 2., 4., t.color("text"))?;
        }
        Ok(())
    }
}
