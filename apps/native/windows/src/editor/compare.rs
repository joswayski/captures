//! Shipping's `CompressionPreview` over the screenshot canvas (`is-embed
//! is-cover`): while Compress or Maximum file size is chosen and Export
//! settings are open, the right of a draggable divider shows the file Save
//! would write and the left the live edit, with Before/After badges, Hide,
//! the round handle and a full-width range along the bottom. A drawing
//! gesture or inline text hides it (`compressComparePaused`), and Hide keeps
//! it hidden until the quality mode changes or Show before / after in the
//! export settings brings it back. `captures_app::compression_compare` owns
//! the copy, badges and bounds; the estimate's encode supplies the After
//! pixels, as on GTK. Deviation: the handle, processing pill and hint draw
//! without `--glass-shadow`, whose 44 px blur shows as rings in the host's
//! banded shadow approximation; like the badges they use solid glass, since
//! Direct2D here has no `backdrop-filter`.
use captures_app::compression_compare as compare;
use captures_app::editor_export::{self, ExportEstimate};
use captures_app::editor_session::{ExportOptions, ExportQuality};
use image::RgbaImage;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::core::*;

use super::{Editor, Event, Image, Layer, Target, bitmap, convert};
use crate::gfx::{self, Style, rect};
use crate::scene::Rect;

/// A control of the comparison, in DOM order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// `.compression-preview-handle`.
    Handle,
    /// `.compression-preview-range`: invisible, along the bottom 28 px.
    Range,
    /// `.compression-preview-dismiss`.
    Hide,
}

pub const PARTS: [Part; 3] = [Part::Handle, Part::Range, Part::Hide];

/// Room around a part for its focus ring and the handle's glass shadow.
pub const BLEED: f32 = 8.;
/// `.compression-preview-range { height: 28px }`.
const RANGE_H: f32 = 28.;
/// `.compression-preview-after-hint`: `max-width: 220px`, `padding: 5px 8px`.
const HINT_W: f32 = 220.;
const HINT_PAD: (f32, f32) = (8., 5.);
/// `AFTER_HINT_PAD`.
const HINT_EDGE: f32 = 8.;

pub struct CompareUi {
    /// `compressSplit`, as a fraction.
    pub split: f64,
    /// `compressCompareDismissed`.
    pub dismissed: bool,
    /// The decoded file Save would write and its bitmap.
    pub after: Option<Image>,
    pub bitmap: Option<ID2D1Bitmap1>,
    /// The handle or range follows the pointer.
    pub dragging: bool,
}

impl Default for CompareUi {
    fn default() -> Self {
        Self {
            split: compare::DEFAULT_SPLIT,
            dismissed: false,
            after: None,
            bitmap: None,
            dragging: false,
        }
    }
}

impl Editor {
    /// Shipping `showCompressCompare`.
    pub fn compare_visible(&self) -> bool {
        self.presented.is_some()
            && compare::screenshot_visible(
                self.options.quality != ExportQuality::Preserve,
                self.export.open,
                self.compare.dismissed,
            )
    }

    /// `compressComparePaused || editingTextId`: faded out so the canvas can
    /// be edited underneath.
    pub(super) fn compare_suppressed(&self) -> bool {
        self.gesture.is_some() || self.inline.is_some()
    }

    /// `isAnnotationDrawTool`: the range yields to the canvas and the After
    /// side shows its hint.
    fn compare_drawing(&self) -> bool {
        !matches!(self.tool, "v" | "c")
    }

    fn compare_processing(&self) -> bool {
        self.export.estimate.pending && !self.compare_suppressed()
    }

    /// The comparison's frame: the image's visible part, well-relative.
    pub(super) fn compare_frame(&self) -> Option<Rect> {
        let r = self.image_rect()?;
        let (w, h) = (self.l.well.w, self.l.well.h);
        let x0 = (r.x as f32).max(0.);
        let y0 = (r.y as f32).max(0.);
        let x1 = ((r.x + r.width) as f32).min(w);
        let y1 = ((r.y + r.height) as f32).min(h);
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// The divider's x, frame-relative.
    fn divider_x(&self, frame: Rect) -> f32 {
        self.image_rect().map_or(frame.w / 2., |r| {
            r.x as f32 + r.width as f32 * self.compare.split as f32 - frame.x
        })
    }

    /// The frame in client DIPs, for UI Automation.
    pub fn compare_rect(&self) -> Rect {
        self.absolute(self.compare_node)
    }

    pub fn compare_part_exists(&self, part: Part) -> bool {
        self.compare_visible() && (part == Part::Hide || self.compare.bitmap.is_some())
    }

    /// Faded out with the frame, the parts take no input
    /// (`is-suppressed { pointer-events: none }`).
    pub fn compare_part_enabled(&self, part: Part) -> bool {
        if self.compare_suppressed() {
            return false;
        }
        match part {
            Part::Handle => !self.compare_processing(),
            Part::Range => !self.compare_processing() && !self.compare_drawing(),
            Part::Hide => true,
        }
    }

    pub fn compare_part_name(part: Part) -> &'static str {
        match part {
            Part::Handle => compare::HANDLE_LABEL,
            Part::Range => compare::RANGE_LABEL,
            Part::Hide => compare::DISMISS_LABEL,
        }
    }

    /// A part's rect, relative to the frame.
    pub(super) fn compare_part_rect(&self, part: Part) -> Rect {
        let Some(frame) = self.compare_frame() else {
            return Rect::default();
        };
        let inset = self.t.n("s-5");
        match part {
            Part::Handle => {
                let side = compare::HANDLE_SIZE as f32;
                Rect::new(
                    self.divider_x(frame) - side / 2.,
                    frame.h / 2. - side / 2.,
                    side,
                    side,
                )
            }
            Part::Range => Rect::new(0., frame.h - RANGE_H, frame.w, RANGE_H),
            Part::Hide => {
                let (w, h) = self.pill_size(compare::DISMISS);
                Rect::new(frame.w - inset - w, inset, w, h)
            }
        }
    }

    /// `.compression-preview-badge` and `-dismiss`: `padding: s-2 s-4`, a
    /// 1 px border, `text-xs`.
    fn pill_size(&self, text: &str) -> (f32, f32) {
        let t = &self.t;
        let text = self.g.text(t, Style::Status, text, 1e4);
        let (w, h) = text.map_or((0., 16.), |t| (t.width, t.height));
        (
            (w + 2. * t.n("s-4") + 2.).ceil(),
            (h + 2. * t.n("s-2") + 2.).ceil(),
        )
    }

    /// Show, hide, fade and redraw the comparison for the current state;
    /// ask for the After side when it is shown without one.
    pub(super) fn sync_compare(&mut self) {
        let visible = self.compare_visible();
        if !visible {
            if self.compare.after.is_none() && !self.nodes[self.compare_node].visible {
                return;
            }
            self.compare.after = None;
            self.compare.bitmap = None;
            self.compare.dragging = false;
            if !self.export.estimating {
                self.export.compare_sent = false;
            }
        } else if self.compare.after.is_none()
            && !self.export.estimating
            && !self.export.compare_sent
        {
            self.schedule_estimate();
        }
        self.place_compare();
        let node = self.compare_node;
        let to = if self.compare_suppressed() { 0. } else { 1. };
        if (self.nodes[node].opacity - to).abs() > 1e-4 {
            self.fade(node, to);
        }
        self.nodes[node].dirty = true;
        for part in PARTS {
            let layered = self.layered(Target::Compare(part));
            for n in [layered.base, layered.lit, layered.ring] {
                self.nodes[n].dirty = true;
            }
        }
        self.relayout = true;
        self.refresh(false);
        self.need = true;
    }

    /// The frame over the image's visible part, or hidden.
    pub(super) fn place_compare(&mut self) {
        let node = self.compare_node;
        let frame = self.compare_frame().filter(|_| self.compare_visible());
        self.set_visible(node, frame.is_some());
        if let Some(frame) = frame {
            self.set_rect(node, frame);
            self.nodes[node].dirty = true;
        }
    }

    /// Redraw the frame (a new estimate is pending or arrived).
    pub(super) fn compare_changed(&mut self) {
        self.nodes[self.compare_node].dirty = true;
        self.need = true;
    }

    /// The After hint follows the pointer.
    pub(super) fn compare_pointer_moved(&mut self) {
        if self.compare_visible() && self.compare_drawing() && self.compare.bitmap.is_some() {
            self.compare_changed();
        }
    }

    /// The estimate's encode, as the After side.
    pub(super) fn receive_after(&mut self, after: Option<Image>) {
        if !self.compare_visible() {
            // Dropped: showing the comparison again asks for it anew.
            self.export.compare_sent = false;
            return;
        }
        let Some(after) = after else {
            return;
        };
        self.compare.bitmap = bitmap(&self.g.dc, &after, (0, 0, after.width, after.height)).ok();
        self.compare.after = Some(after);
        self.events.push(Event::Structure);
    }

    /// The quality mode changed: a hidden comparison comes back.
    pub(super) fn compare_quality_changed(&mut self, previous: ExportQuality) {
        if previous == self.options.quality {
            return;
        }
        self.compare.dismissed = false;
        self.compare.after = None;
        self.compare.bitmap = None;
        if self.options.quality == ExportQuality::Preserve {
            self.compare.split = compare::DEFAULT_SPLIT;
        }
    }

    pub(super) fn activate_compare(&mut self, part: Part) {
        if part != Part::Hide {
            return;
        }
        self.compare.dismissed = true;
        if self.focus == Some(Target::Compare(Part::Hide)) {
            self.set_focus(None, false);
        }
        self.events.push(Event::Structure);
        self.export_dirty();
    }

    /// Show before / after in the export settings.
    pub(super) fn show_compare(&mut self) {
        self.compare.dismissed = false;
        if self.focus == Some(Target::Prop(super::props::Prop::Export(super::Ex::Compare))) {
            self.set_focus(Some(Target::ExportSettings), self.focus_visible);
        }
        self.events.push(Event::Structure);
        self.export_dirty();
    }

    pub fn compare_split(&self) -> f64 {
        self.compare.split
    }

    pub fn set_compare_split(&mut self, split: f64) {
        let split = compare::clamp_split(split);
        if (split - self.compare.split).abs() < 1e-9 {
            return;
        }
        self.compare.split = split;
        self.events.push(Event::Value(Target::Compare(Part::Range)));
        self.sync_compare();
    }

    /// A press on the handle or the range: follow the pointer.
    pub(super) fn press_compare(&mut self, part: Part, x: f32) {
        if part == Part::Range {
            self.set_focus(Some(Target::Compare(part)), false);
        }
        self.compare.dragging = true;
        self.drag_compare(x);
    }

    pub(super) fn drag_compare(&mut self, x: f32) {
        let Some(r) = self.image_rect() else {
            return;
        };
        let local = f64::from(x - self.l.well.x) - r.x;
        self.set_compare_split(compare::split_at(local, r.width));
    }

    /// Arrow keys, Page Up/Down, Home and End on the focused range.
    pub(super) fn compare_key(&mut self, key: VIRTUAL_KEY) -> bool {
        let range = Target::Compare(Part::Range);
        if self.focus != Some(range) || !self.enabled(range) {
            return false;
        }
        use compare::SplitKey::*;
        let key = match key {
            VK_LEFT | VK_DOWN => Decrease,
            VK_RIGHT | VK_UP => Increase,
            VK_PRIOR => PageUp,
            VK_NEXT => PageDown,
            VK_HOME => Home,
            VK_END => End,
            _ => return false,
        };
        self.set_compare_split(compare::keyboard_split(self.compare.split, key));
        true
    }

    /// Whether a press at a client point lands on the processing veil
    /// (`cursor: wait`), which takes it.
    pub(super) fn compare_blocks(&self, x: f32, y: f32) -> bool {
        self.compare_processing()
            && self.compare_visible()
            && self
                .compare_frame()
                .is_some_and(|f| f.contains(x - self.l.well.x, y - self.l.well.y))
    }

    // ---- painting ----

    /// The After side, the divider, the veil, the badges, the processing
    /// pill and the After hint, frame-relative.
    pub(super) fn draw_compare(&self, dc: &ID2D1DeviceContext, (w, h): (f32, f32)) -> Result<()> {
        let Some(frame) = self.compare_frame() else {
            return Ok(());
        };
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let edge = self.divider_x(frame);
        if let (Some(bitmap), Some(r)) = (&self.compare.bitmap, self.image_rect()) {
            let image = rect(
                r.x as f32 - frame.x,
                r.y as f32 - frame.y,
                (r.x + r.width) as f32 - frame.x,
                (r.y + r.height) as f32 - frame.y,
            );
            unsafe {
                dc.PushAxisAlignedClip(&rect(edge, 0., w, h), D2D1_ANTIALIAS_MODE_ALIASED);
                dc.DrawBitmap(
                    bitmap,
                    Some(&image),
                    1.,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
                dc.PopAxisAlignedClip();
            }
            // `.compression-preview-divider`: 2 px with a 1 px dark ring.
            gfx::fill(
                dc,
                rect(edge - 2., 0., edge + 2., h),
                D2D1_COLOR_F {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: 0.35,
                },
            )?;
            gfx::fill(dc, rect(edge - 1., 0., edge + 1., h), c("glass-text"))?;
        }
        if self.export.estimate.pending {
            self.draw_veil(dc, (w, h))?;
        }
        let badges = compare::badges(
            self.export.estimate.baseline_bytes,
            self.export.estimate.bytes,
            self.compare_processing(),
        );
        let inset = t.n("s-5");
        let (bw, bh) = self.pill_size(&badges.before);
        self.draw_pill(
            dc,
            &badges.before,
            "",
            rect(inset, h - inset - bh, inset + bw, h - inset),
            false,
        )?;
        let savings = badges.savings.as_deref().unwrap_or_default();
        let (aw, ah) = self.pill_size(&format!("{}{savings}", badges.after));
        self.draw_pill(
            dc,
            &badges.after,
            savings,
            rect(w - inset - aw, h - inset - ah, w - inset, h - inset),
            false,
        )?;
        if self.compare_processing() {
            self.draw_processing(dc, (w, h))?;
        }
        self.draw_after_hint(dc, frame, edge, (w, h))
    }

    /// `.compression-preview-veil`: a radial `glass-veil-soft` →
    /// `glass-veil` (62 %) → `glass-veil-heavy` wash.
    fn draw_veil(&self, dc: &ID2D1DeviceContext, (w, h): (f32, f32)) -> Result<()> {
        let t = &self.t;
        let stops = [
            (0., t.color("glass-veil-soft")),
            (0.62, t.color("glass-veil")),
            (1., t.color("glass-veil-heavy")),
        ]
        .map(|(position, color)| D2D1_GRADIENT_STOP { position, color });
        unsafe {
            let target: &ID2D1RenderTarget = dc;
            let collection = target.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )?;
            // `ellipse at center` reaches the farthest corner.
            let brush = target.CreateRadialGradientBrush(
                &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: windows_numerics::Vector2 {
                        X: w / 2.,
                        Y: h / 2.,
                    },
                    gradientOriginOffset: windows_numerics::Vector2::default(),
                    radiusX: w / 2. * std::f32::consts::SQRT_2,
                    radiusY: h / 2. * std::f32::consts::SQRT_2,
                },
                None,
                &collection,
            )?;
            dc.FillRectangle(&rect(0., 0., w, h), &brush);
        }
        Ok(())
    }

    /// A glass pill: a badge, or Hide (`raised` while hovered).
    fn draw_pill(
        &self,
        dc: &ID2D1DeviceContext,
        text: &str,
        savings: &str,
        r: D2D_RECT_F,
        raised: bool,
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let radius = (r.bottom - r.top) / 2.;
        // Without `backdrop-filter`, the solid glass keeps text readable.
        gfx::fill_round(
            dc,
            r,
            radius,
            c(if raised {
                "glass-raised-solid"
            } else {
                "glass-strong-solid"
            }),
        )?;
        gfx::border(
            dc,
            r,
            radius,
            1.,
            c(if raised {
                "glass-border-strong"
            } else {
                "glass-border"
            }),
        )?;
        let label = self.g.text(t, Style::Status, text, 1e4)?;
        let y = ((r.top + r.bottom - label.height) / 2.).round();
        let x = r.left + 1. + t.n("s-4");
        gfx::text(dc, &label, x, y, c("glass-text"))?;
        if !savings.is_empty() {
            let more = self.g.text(t, Style::Status, savings, 1e4)?;
            gfx::text(dc, &more, x + label.width, y, c("positive-text"))?;
        }
        Ok(())
    }

    /// `.compression-preview-processing`: the centred pill with its spinner.
    fn draw_processing(&self, dc: &ID2D1DeviceContext, (w, h): (f32, f32)) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let (s4, s6) = (t.n("s-4"), t.n("s-6"));
        let label = self.g.text(t, Style::Control, compare::PROCESSING, 1e4)?;
        let spinner = 16.;
        let pw = (2. + 2. * s6 + spinner + s4 + label.width).ceil();
        let ph = (2. + 2. * s4 + label.height.max(spinner)).ceil();
        let (x, y) = (((w - pw) / 2.).round(), ((h - ph) / 2.).round());
        let pill = rect(x, y, x + pw, y + ph);
        gfx::fill_round(dc, pill, ph / 2., c("glass-strong-solid"))?;
        gfx::border(dc, pill, ph / 2., 1., c("glass-border"))?;
        // `border: 2px solid; border-right-color: transparent`, at rest
        // (all four sides with reduced motion, as shipping draws it).
        let (cx, cy) = (x + 1. + s6 + spinner / 2., y + ph / 2.);
        let radius = spinner / 2. - 1.;
        let brush = gfx::brush(dc, c("glass-text"))?;
        unsafe {
            if self.reduced_motion {
                dc.DrawEllipse(
                    &D2D1_ELLIPSE {
                        point: windows_numerics::Vector2 { X: cx, Y: cy },
                        radiusX: radius,
                        radiusY: radius,
                    },
                    &brush,
                    2.,
                    None,
                );
            } else {
                let geometry = self.g.factory.CreatePathGeometry()?;
                let sink = geometry.Open()?;
                let at = |deg: f32| {
                    let a = deg.to_radians();
                    windows_numerics::Vector2 {
                        X: cx + radius * a.cos(),
                        Y: cy + radius * a.sin(),
                    }
                };
                // From the top-right diagonal round to the bottom-right one.
                sink.BeginFigure(at(-45.), D2D1_FIGURE_BEGIN_HOLLOW);
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: at(45.),
                    size: D2D_SIZE_F {
                        width: radius,
                        height: radius,
                    },
                    rotationAngle: 0.,
                    sweepDirection: D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
                    arcSize: D2D1_ARC_SIZE_LARGE,
                });
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
                sink.Close()?;
                dc.DrawGeometry(&geometry, &brush, 2., None);
            }
        }
        let tx = x + 1. + s6 + spinner + s4;
        gfx::text(
            dc,
            &label,
            tx,
            (y + (ph - label.height) / 2.).round(),
            c("glass-text"),
        )
    }

    /// `.compression-preview-after-hint`, following the pointer over the
    /// After side while a drawing tool is chosen.
    fn draw_after_hint(
        &self,
        dc: &ID2D1DeviceContext,
        frame: Rect,
        edge: f32,
        (w, h): (f32, f32),
    ) -> Result<()> {
        if !self.compare_drawing() || self.compare.bitmap.is_none() || self.compare.dragging {
            return Ok(());
        }
        let Some((px, py)) = self.pointer else {
            return Ok(());
        };
        let (lx, ly) = (px - self.l.well.x - frame.x, py - self.l.well.y - frame.y);
        if lx <= edge || lx > w || ly < 0. || ly > h {
            return Ok(());
        }
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let max = HINT_W.min(w - 16.).max(1.) - 2. - 2. * HINT_PAD.0;
        let text = self.g.text(t, Style::Status, compare::AFTER_HINT, max)?;
        let (bw, bh) = (
            (text.width + 2. * HINT_PAD.0 + 2.).ceil(),
            (text.height + 2. * HINT_PAD.1 + 2.).ceil(),
        );
        let (x, y) = after_hint_position((lx, ly), (w, h), (bw, bh));
        let r = rect(x, y, x + bw, y + bh);
        let radius = t.n("r-sm");
        gfx::fill_round(dc, r, radius, c("glass-strong-solid"))?;
        gfx::border(dc, r, radius, 1., c("glass-border"))?;
        gfx::text(
            dc,
            &text,
            x + 1. + HINT_PAD.0,
            y + 1. + HINT_PAD.1,
            c("glass-text"),
        )
    }

    /// `.screenshot-show-comparison`'s width.
    pub(super) fn compare_button_w(&self) -> f32 {
        let t = &self.t;
        let text = self.g.text(t, Style::Button, compare::SHOW, 1e4);
        (text.map_or(0., |t| t.width) + 2. * t.n("s-4") + 2.).ceil()
    }

    /// Show before / after: `.screenshot-show-comparison`.
    pub(super) fn draw_show_compare(
        &self,
        dc: &ID2D1DeviceContext,
        layer: Layer,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let radius = t.n("r-md");
        let lit = layer == Layer::Lit;
        gfx::fill_round(
            dc,
            all,
            radius,
            c(if lit { "control-hover" } else { "control" }),
        )?;
        gfx::border(
            dc,
            all,
            radius,
            1.,
            c(if lit {
                "border-strong"
            } else {
                "control-border"
            }),
        )?;
        let text = self.g.text(t, Style::Button, compare::SHOW, 1e4)?;
        gfx::text(
            dc,
            &text,
            ((w - text.width) / 2.).round(),
            ((h - text.height) / 2.).round(),
            c("text"),
        )
    }

    /// The handle, the (invisible) range, and Hide.
    pub(super) fn draw_compare_part(
        &self,
        dc: &ID2D1DeviceContext,
        part: Part,
        layer: Layer,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        match (part, layer) {
            // `opacity: 0`; its keyboard focus shows on the handle.
            (Part::Range, _) => Ok(()),
            (Part::Handle, Layer::Ring) => gfx::ring(dc, all, h / 2., 2., 4., c("theme-accent")),
            (Part::Handle, Layer::Base) => {
                gfx::fill_round(dc, all, h / 2., c("glass-strong-solid"))?;
                gfx::border(dc, all, h / 2., 1., c("glass-border-strong"))?;
                let glyph = self.g.text(t, Style::Button, compare::HANDLE_GLYPH, 1e4)?;
                gfx::text(
                    dc,
                    &glyph,
                    ((w - glyph.width) / 2.).round(),
                    ((h - glyph.height) / 2.).round(),
                    c("glass-text"),
                )
            }
            (Part::Handle, Layer::Lit) => Ok(()),
            (Part::Hide, Layer::Ring) => gfx::ring(dc, all, h / 2., 2., 4., c("theme-accent")),
            (Part::Hide, layer) => {
                self.draw_pill(dc, compare::DISMISS, "", all, layer == Layer::Lit)
            }
        }
    }
}

/// The estimate, and with the comparison shown the decoded encode as its
/// After side; one encode serves both, as on GTK. Compression modes compare
/// with the lossless flattened image either way.
pub(super) fn estimate(
    pixels: &RgbaImage,
    options: ExportOptions,
    original: Option<u64>,
    comparing: bool,
) -> (std::result::Result<ExportEstimate, String>, Option<Image>) {
    if !comparing {
        return (
            editor_export::estimate_export(pixels, options, original),
            None,
        );
    }
    let encoded = compare::encode_after(pixels, options).and_then(|(after, bytes)| {
        let baseline = captures_history::encode_png(pixels)
            .map_err(|error| error.to_string())?
            .len() as u64;
        Ok((
            ExportEstimate {
                bytes,
                baseline_bytes: Some(baseline),
            },
            after,
        ))
    });
    match encoded {
        Ok((estimate, after)) => {
            let (width, height) = after.dimensions();
            let bgra = convert(&after, (0, 0, width, height));
            (
                Ok(estimate),
                Some(Image {
                    width,
                    height,
                    bgra,
                }),
            )
        }
        Err(error) => (Err(error), None),
    }
}

/// `compressionAfterHintPosition`: below right of the pointer, flipped and
/// then held inside the frame.
fn after_hint_position(
    (x, y): (f32, f32),
    (fw, fh): (f32, f32),
    (hw, hh): (f32, f32),
) -> (f32, f32) {
    let w = hw.min((fw - HINT_EDGE * 2.).max(1.));
    let h = hh.min((fh - HINT_EDGE * 2.).max(1.));
    let mut left = x + 12.;
    let mut top = y + 14.;
    if left + w + HINT_EDGE > fw {
        left = x - w - HINT_EDGE;
    }
    if top + h + HINT_EDGE > fh {
        top = y - h - HINT_EDGE;
    }
    (
        left.max(HINT_EDGE).min(fw - w - HINT_EDGE),
        top.max(HINT_EDGE).min(fh - h - HINT_EDGE),
    )
}
