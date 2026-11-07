//! Accepted-session comparison. A superseded encode cannot repaint newer edits.
use std::sync::Arc;
use std::time::{Duration, Instant};

use captures_app::compression_compare as ui_compare;

use super::preview::Frame;
use super::*;

pub(super) struct Encoded {
    pub revision: u64,
    pub position_ms: u64,
    pub before: Arc<Frame>,
    pub after: Arc<Frame>,
    pub bytes: u64,
}

pub(super) struct Compare {
    pub generation: u64,
    pub due: Option<Instant>,
    pub pending: bool,
    pub dismissed: bool,
    pub split: f64,
    pub dragging: bool,
    pub encoded: Option<Encoded>,
    bitmaps: Option<(ID2D1Bitmap1, ID2D1Bitmap1)>,
    error: Option<String>,
}

impl Default for Compare {
    fn default() -> Self {
        Self {
            generation: 0,
            due: None,
            pending: false,
            dismissed: false,
            split: ui_compare::DEFAULT_SPLIT,
            dragging: false,
            encoded: None,
            bitmaps: None,
            error: None,
        }
    }
}

impl Recording {
    pub(super) fn compresses(&self) -> bool {
        self.presented.is_some()
            && (self.extension() == "gif" || self.output.values.mode == ui::QualityMode::Compress)
    }

    pub(super) fn compare_visible(&self) -> bool {
        ui_compare::recording_visible(
            self.compresses(),
            self.preview.playing && self.extension() != "gif",
            self.compare.dismissed,
        ) && !self.closing
            && !self.saving
    }

    pub(super) fn cancel_compare(&mut self) {
        if self
            .compare
            .error
            .as_ref()
            .is_some_and(|e| self.status == format!("Comparison unavailable: {e}"))
        {
            self.notify("", false);
        }
        if let Some(cancel) = self.worker.comparison_cancel.take() {
            cancel.cancel();
        }
        self.compare.generation += 1;
        self.compare.due = None;
        self.compare.pending = false;
        self.compare.dragging = false;
        self.compare.encoded = None;
        self.compare.bitmaps = None;
        self.compare.error = None;
        if !self.hwnd.is_invalid() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), COMPARE_TIMER);
            }
        }
        self.dirty[PREVIEW] = true;
        self.events.push(Event::Structure);
    }

    pub(super) fn schedule_compare(&mut self) {
        self.cancel_compare();
        if !self.compare_visible() || self.timeline.pending {
            self.place_play();
            return;
        }
        // GIF's source animation runs underneath Tauri's static comparison.
        // Stop its native decoder so the single session worker can encode.
        if self.extension() == "gif" {
            self.stop_preview();
            self.timeline.resume = false;
        }
        self.compare.pending = true;
        self.compare.due =
            Some(Instant::now() + Duration::from_millis(ui_compare::REFRESH_DELAY_MS));
        if !self.hwnd.is_invalid() {
            unsafe {
                SetTimer(
                    Some(self.hwnd),
                    COMPARE_TIMER,
                    ui_compare::REFRESH_DELAY_MS as u32,
                    None,
                );
            }
        }
        self.place_play();
    }

    pub(super) fn compare_due(&mut self) {
        if self.compare.due.is_none_or(|due| due > Instant::now()) {
            return;
        }
        self.compare.due = None;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), COMPARE_TIMER);
        }
        if !self.compare_visible() || self.timeline.pending {
            self.cancel_compare();
            return;
        }
        let cancel = CancelToken::default();
        self.worker.comparison_cancel = Some(cancel.clone());
        if self
            .worker
            .jobs
            .send(Job::Compare(self.compare.generation, cancel))
            .is_err()
        {
            self.accept_compare(
                self.compare.generation,
                Err("The recording session closed.".into()),
            );
        }
    }

    pub(super) fn accept_compare(
        &mut self,
        generation: u64,
        result: std::result::Result<Encoded, String>,
    ) {
        if generation != self.compare.generation || !self.compare_visible() || self.timeline.pending
        {
            return;
        }
        self.worker.comparison_cancel.take();
        self.compare.pending = false;
        let result = result.and_then(|encoded| {
            let p = self.presented.as_ref().expect("visible comparison");
            if encoded.revision != p.revision || encoded.position_ms != p.position_ms {
                return Err("The comparison no longer matches the selected frame.".into());
            }
            let before = encoded
                .before
                .bitmap(&self.g.dc)
                .map_err(|e| e.to_string())?;
            let after = encoded
                .after
                .bitmap(&self.g.dc)
                .map_err(|e| e.to_string())?;
            self.compare.bitmaps = Some((before, after));
            self.compare.encoded = Some(encoded);
            Ok(())
        });
        if let Err(error) = result {
            self.compare.error = Some(error.clone());
            self.notify(&format!("Comparison unavailable: {error}"), true);
        }
        self.dirty[PREVIEW] = true;
        self.place_play();
        self.events.push(Event::Structure);
    }

    pub fn compare_range(&self, target: Target) -> Option<(f64, f64, f64)> {
        (target == Target::CompareSplit).then_some((
            self.compare.split * 100.,
            ui_compare::MIN_SPLIT * 100.,
            ui_compare::MAX_SPLIT * 100.,
        ))
    }

    pub fn set_compare_split(&mut self, value: f64) {
        if !value.is_finite() || !self.enabled(Target::CompareSplit) {
            return;
        }
        self.compare.split = ui_compare::clamp_split(value / 100.);
        self.layout_compare();
        self.dirty[PREVIEW] = true;
        self.events.push(Event::Range(Target::CompareSplit));
    }

    pub(super) fn layout_compare(&mut self) {
        let r = self.preview_media();
        let h = ui_compare::HANDLE_SIZE as f32;
        self.rects[slot(Target::CompareSplit)] = Rect::new(
            r.x + r.w * self.compare.split as f32 - h / 2.,
            r.y + r.h / 2. - h / 2.,
            h,
            h,
        );
        self.rects[slot(Target::CompareHide)] = Rect::new(r.x + r.w - 66., r.y + 12., 54., 28.);
        for t in [Target::CompareHide, Target::CompareSplit] {
            self.dirty[slot(t)] = true;
        }
    }

    pub(super) fn activate_compare(&mut self, target: Target) {
        self.compare.dismissed = target == Target::CompareHide;
        if !self.compare.dismissed && self.extension() == "gif" && self.preview.playing {
            self.stop_preview();
            self.seek_timeline(self.preview.position_ms);
        }
        self.schedule_compare();
        if self.compare.dismissed && self.extension() == "gif" {
            self.start_preview();
        }
        let _ = self.layout();
        // Hide removes its own node; preserve a useful keyboard focus target.
        if self.focus == Some(target) {
            self.set_focus(
                Some(if self.compare.dismissed {
                    Target::CompareShow
                } else {
                    Target::CompareHide
                }),
                self.focus_visible,
            );
        }
    }

    pub(super) fn compare_key(&mut self, key: VIRTUAL_KEY) -> bool {
        if key == VK_ESCAPE && self.compare.dragging {
            self.compare.dragging = false;
            unsafe {
                let _ = ReleaseCapture();
            }
            return true;
        }
        if self.focus != Some(Target::CompareSplit) || !self.enabled(Target::CompareSplit) {
            return false;
        }
        use ui_compare::SplitKey::*;
        let key = match key {
            VK_LEFT | VK_DOWN => Decrease,
            VK_RIGHT | VK_UP => Increase,
            VK_PRIOR => PageUp,
            VK_NEXT => PageDown,
            VK_HOME => Home,
            VK_END => End,
            _ => return false,
        };
        self.set_compare_split(ui_compare::keyboard_split(self.compare.split, key) * 100.);
        true
    }

    pub(super) fn compare_pointer(&mut self, msg: u32, x: f32, y: f32) -> bool {
        let media = self.preview_media();
        let starts = msg == WM_LBUTTONDOWN
            && self.enabled(Target::CompareSplit)
            && self.crop_hit(x, y).is_none()
            && self.page_visible_rect(self.preview_view()).contains(x, y)
            && media.contains(x, y)
            && !self.target_rect(Target::CompareHide).contains(x, y)
            && !(self.shown(Target::Play) && self.target_rect(Target::Play).contains(x, y));
        if starts {
            self.compare.dragging = true;
            self.set_focus(Some(Target::CompareSplit), false);
            unsafe {
                let _ = SetFocus(Some(self.hwnd));
                SetCapture(self.hwnd);
            }
        }
        if !self.compare.dragging {
            return false;
        }
        self.set_compare_split(ui_compare::split_at((x - media.x).into(), media.w.into()) * 100.);
        if msg == WM_LBUTTONUP {
            self.compare.dragging = false;
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        true
    }

    pub(super) fn draw_compare(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        if !self.compare_visible() {
            return Ok(());
        }
        let base = self.rects[PREVIEW];
        let media = self.preview_media();
        let r = Rect::new(media.x - base.x, media.y - base.y, media.w, media.h);
        let view = self.preview_view();
        let clip = gfx::rect(
            view.x - base.x,
            view.y - base.y,
            view.x + view.w - base.x,
            view.y + view.h - base.y,
        );
        unsafe {
            dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
        }
        let result = (|| {
            let edge = r.x + r.w * self.compare.split as f32;
            if let Some((before, after)) = &self.compare.bitmaps {
                gfx::fill(
                    dc,
                    gfx::rect(r.x, r.y, r.x + r.w, r.y + r.h),
                    self.t.color("glass-strong-solid"),
                )?;
                for (bitmap, left, right) in [(before, r.x, edge), (after, edge, r.x + r.w)] {
                    unsafe {
                        dc.PushAxisAlignedClip(
                            &gfx::rect(left, r.y, right, r.y + r.h),
                            D2D1_ANTIALIAS_MODE_ALIASED,
                        );
                        dc.DrawBitmap(
                            bitmap,
                            Some(&gfx::rect(r.x, r.y, r.x + r.w, r.y + r.h)),
                            1.,
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            None,
                            None,
                        );
                        dc.PopAxisAlignedClip();
                    }
                }
                gfx::fill(
                    dc,
                    gfx::rect(edge - 1., r.y, edge + 1., r.y + r.h),
                    self.t.color("glass-text"),
                )?;
            }
            let badges = ui_compare::badges(
                self.presented.as_ref().map(|p| p.size_bytes),
                self.compare.encoded.as_ref().map(|e| e.bytes),
                self.compare.pending,
            );
            let pad = self.t.n("s-4");
            let inset = self.t.n("s-5");
            let pill = |text: &str, savings: &str, area: Rect| -> Result<()> {
                gfx::fill_round(
                    dc,
                    gfx::rect(area.x, area.y, area.x + area.w, area.y + area.h),
                    self.t.n("r-pill").min(area.h / 2.),
                    self.t.color("glass-strong-solid"),
                )?;
                let text_width = self
                    .g
                    .text(&self.t, Style::Tiny, text, area.w)?
                    .width
                    .min((area.w - 2. * pad).max(1.));
                self.draw_text(
                    dc,
                    Style::Tiny,
                    text,
                    Rect::new(area.x + pad, area.y, text_width, area.h),
                    "glass-text",
                    false,
                )?;
                if !savings.is_empty() && area.w > text_width + 2. * pad {
                    self.draw_text(
                        dc,
                        Style::Tiny,
                        savings,
                        Rect::new(
                            area.x + pad + text_width,
                            area.y,
                            area.w - text_width - 2. * pad,
                            area.h,
                        ),
                        "positive-text",
                        false,
                    )?;
                }
                Ok(())
            };
            let width = |text: &str| -> Result<f32> {
                Ok(
                    (self.g.text(&self.t, Style::Tiny, text, 1000.)?.width + 2. * pad)
                        .min((r.w - 2. * inset).max(1.)),
                )
            };
            let savings = badges.savings.as_deref().unwrap_or_default();
            let after_width = width(&format!("{}{savings}", badges.after))?;
            pill(
                &badges.before,
                "",
                Rect::new(
                    r.x + inset,
                    r.y + r.h - inset - 28.,
                    width(&badges.before)?,
                    28.,
                ),
            )?;
            pill(
                &badges.after,
                savings,
                Rect::new(
                    r.x + r.w - after_width - inset,
                    r.y + r.h - inset - 28.,
                    after_width,
                    28.,
                ),
            )?;
            if self.compare.pending || self.compare.error.is_some() {
                let text = self
                    .compare
                    .error
                    .as_deref()
                    .unwrap_or(ui_compare::PROCESSING);
                let width = (r.w - 24.).clamp(1., 420.);
                pill(
                    text,
                    "",
                    Rect::new(r.x + (r.w - width) / 2., r.y + r.h / 2. - 14., width, 28.),
                )?;
            }
            Ok(())
        })();
        unsafe {
            dc.PopAxisAlignedClip();
        }
        result
    }

    pub(super) fn draw_compare_control(
        &self,
        dc: &ID2D1DeviceContext,
        target: Target,
    ) -> Result<()> {
        let r = self.target_rect(target);
        let local = gfx::rect(0., 0., r.w, r.h);
        gfx::fill_round(dc, local, r.h / 2., self.t.color("glass-strong-solid"))?;
        if self.hover == Some(target) {
            gfx::fill_round(dc, local, r.h / 2., self.t.color("glass-hover"))?;
        }
        let text = if target == Target::CompareSplit {
            ui_compare::HANDLE_GLYPH
        } else {
            ui_compare::DISMISS
        };
        let measured = self.g.text(&self.t, Style::Control, text, r.w)?;
        gfx::text(
            dc,
            &measured,
            (r.w - measured.width) / 2.,
            (r.h - measured.height) / 2.,
            self.t.color("glass-text"),
        )?;
        if self.focus == Some(target) && self.focus_visible {
            gfx::ring(dc, local, r.h / 2., 2., 4., self.t.color("theme-accent"))?;
        }
        Ok(())
    }
}
