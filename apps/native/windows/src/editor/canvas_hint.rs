//! Shipping's canvas size hints over the page: the Trim edges hover preview
//! (`.screenshot-canvas-trim-hint`), shown while Trim edges is hovered or
//! focused, and the Expand canvas action for a selected layer hanging past
//! the canvas, with its overflow tint and, while the action is hovered, the
//! dashed ghost of the grown canvas and its edge blooms. The shared
//! `trim_preview` and `canvas_expand_preview` own the geometry and the
//! `TRIM_*` / `SNAP_*` constants the look, as for GTK and AppKit, which also
//! tint the overflow rather than repainting the layer's off-canvas pixels;
//! the breathing and particle animations are omitted, as GTK omits them.
use captures_app::editor::{LayerEdit, Point, Rect as DocRect};
use captures_app::editor_canvas::{
    CanvasEdge, CanvasExpandPreview, CanvasTrimPreview, EXPAND_ACTION_INSET, EXPAND_CANVAS,
    SNAP_BLOOM, SNAP_BLOOM_STOPS, SNAP_EDGE_BAR, TRIM_BLOOM, TRIM_BLOOM_STOPS, TRIM_EDGE_BAR,
    TRIM_KEEP_ALPHA, TRIM_KEEP_GLOW_ALPHA, TRIM_KEEP_RADIUS, TRIM_KEEP_WIDTH, TRIM_REGION_ALPHA,
    TRIM_RGB, inset_anchor,
};
use captures_app::editor_session::Request;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::*;

use super::import::hint_bloom;
use super::{Editor, Layer, Target};
use crate::gfx::{self, Style, rect};
use crate::scene::Rect;

/// `--trim-rgb` at `alpha`.
fn trim_red(alpha: f64) -> D2D1_COLOR_F {
    let [r, g, b] = TRIM_RGB.map(|c| f32::from(c) / 255.);
    D2D1_COLOR_F {
        r,
        g,
        b,
        a: alpha as f32,
    }
}

/// A CSS `box-shadow: inset 0 0 <blur> <color>` on a square box, as 2 px
/// bands inside it following the Gaussian tail, like `gfx::glow` outside.
fn inset_glow(
    dc: &ID2D1DeviceContext,
    r: D2D_RECT_F,
    blur: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    let room = ((r.right - r.left).min(r.bottom - r.top) / 2.).max(0.);
    let sigma = (blur / 2.).max(0.5);
    let band = 2.;
    let mut d = 0.;
    while d < blur.min(room) {
        let z = (d + band / 2.) / sigma / std::f32::consts::SQRT_2;
        let a = 0.147;
        let z2 = z * z;
        let erf = (1. - (-z2 * (4. / std::f32::consts::PI + a * z2) / (1. + a * z2)).exp()).sqrt();
        let alpha = color.a * 0.5 * (1. - erf);
        let to = (d + band).min(room);
        gfx::ring(dc, r, 0., -to, -d, D2D1_COLOR_F { a: alpha, ..color })?;
        d += band;
    }
    Ok(())
}

/// An edge light's look: shipping's `-expand-edge` (accent) and
/// `-trim-edge` (red) share its shape.
#[derive(Clone, Copy)]
pub(super) struct Light {
    pub color: D2D1_COLOR_F,
    /// The pill's thickness and the bloom's depth.
    pub bar: f32,
    pub depth: f32,
    pub stops: [(f64, f64); 4],
    /// The pill's `box-shadow` glows: blur and alpha.
    pub halo: &'static [(f32, f32)],
}

/// The pill straddling `edge` of `b` (1 px past each end) with its glows,
/// and the bloom outward from it.
pub(super) fn edge_light(
    dc: &ID2D1DeviceContext,
    b: D2D_RECT_F,
    edge: CanvasEdge,
    light: &Light,
) -> Result<()> {
    let Light {
        color,
        bar,
        depth,
        stops,
        halo,
    } = *light;
    // Shipping offsets each `::after` 2 px out from the edge.
    let (pill, bloom, angle) = match edge {
        CanvasEdge::Top => (
            rect(b.left - 1., b.top - 2., b.right + 1., b.top - 2. + bar),
            rect(b.left, b.top - depth, b.right, b.top),
            0.,
        ),
        CanvasEdge::Bottom => (
            rect(
                b.left - 1.,
                b.bottom + 2. - bar,
                b.right + 1.,
                b.bottom + 2.,
            ),
            rect(b.left, b.bottom, b.right, b.bottom + depth),
            180.,
        ),
        CanvasEdge::Left => (
            rect(b.left - 2., b.top - 1., b.left - 2. + bar, b.bottom + 1.),
            rect(b.left - depth, b.top, b.left, b.bottom),
            270.,
        ),
        CanvasEdge::Right => (
            rect(b.right + 2. - bar, b.top - 1., b.right + 2., b.bottom + 1.),
            rect(b.right, b.top, b.right + depth, b.bottom),
            90.,
        ),
    };
    hint_bloom(dc, color, bloom, angle, stops)?;
    for &(blur, alpha) in halo.iter().rev() {
        gfx::glow(
            dc,
            pill,
            bar / 2.,
            blur,
            D2D1_COLOR_F {
                a: color.a * alpha,
                ..color
            },
        )?;
    }
    gfx::fill_round(dc, pill, bar / 2., color)
}

impl Editor {
    /// The Trim edges preview to draw: while the enabled button is hovered
    /// or focused (shipping's `onPointerEnter` / `onFocus`).
    pub(super) fn trim_hint(&self) -> Option<&CanvasTrimPreview> {
        let on = self.hover == Some(Target::Trim) || self.focus == Some(Target::Trim);
        if !on || !self.enabled(Target::Trim) {
            return None;
        }
        self.presented.as_ref()?.trim_preview.as_ref()
    }

    /// A document rectangle in well DIPs.
    fn well_rect(&self, r: DocRect) -> Option<D2D_RECT_F> {
        let a = self.to_well(Point { x: r.x, y: r.y })?;
        let b = self.to_well(Point {
            x: r.x + r.width,
            y: r.y + r.height,
        })?;
        Some(rect(a.0, a.1, b.0, b.1))
    }

    /// `.screenshot-canvas-trim-hint`: the discarded margins tinted red, the
    /// dashed rectangle that stays, and a red pill with an outward bloom on
    /// each cut edge.
    pub(super) fn draw_trim_hint(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let Some(preview) = self.trim_hint() else {
            return Ok(());
        };
        for (_, region) in &preview.regions {
            let Some(r) = self.well_rect(*region) else {
                continue;
            };
            gfx::fill(dc, r, trim_red(TRIM_REGION_ALPHA))?;
            inset_glow(dc, r, 24., trim_red(0.12))?;
        }
        let Some(keep) = self.well_rect(preview.keep) else {
            return Ok(());
        };
        let radius = TRIM_KEEP_RADIUS as f32;
        // `0 0 0 1px .1, 0 0 22px .18` and the 1.5 px dashed border.
        gfx::glow(dc, keep, radius, 22., trim_red(TRIM_KEEP_GLOW_ALPHA))?;
        gfx::ring(dc, keep, radius, 0., 1., trim_red(0.1))?;
        let width = TRIM_KEEP_WIDTH as f32;
        let inset = width / 2.;
        unsafe {
            // A CSS dashed border's dashes are about three widths long.
            let style = self.g.factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES1 {
                    dashStyle: D2D1_DASH_STYLE_CUSTOM,
                    miterLimit: 10.,
                    ..Default::default()
                },
                Some(&[3., 3.]),
            )?;
            dc.DrawRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(
                        keep.left + inset,
                        keep.top + inset,
                        keep.right - inset,
                        keep.bottom - inset,
                    ),
                    radiusX: (radius - inset).max(0.),
                    radiusY: (radius - inset).max(0.),
                },
                &gfx::brush(dc, trim_red(TRIM_KEEP_ALPHA))?,
                width,
                &style,
            );
        }
        for edge in &preview.edges {
            edge_light(
                dc,
                keep,
                *edge,
                &Light {
                    color: trim_red(1.),
                    bar: TRIM_EDGE_BAR as f32,
                    depth: TRIM_BLOOM as f32,
                    stops: TRIM_BLOOM_STOPS,
                    halo: &[(8., 0.95), (20., 0.55), (32., 0.32)],
                },
            )?;
        }
        Ok(())
    }
}

/// `.screenshot-drop-snap-guide::after` / `.screenshot-canvas-expand-edge::after`
/// glows.
const SNAP_HALO: &[(f32, f32)] = &[(8., 0.95), (20., 0.65), (36., 0.4)];

impl Editor {
    /// The layer the Expand canvas action is for: the selected, visible
    /// layer while it hangs past the canvas, with a transform tool and no
    /// drag or open label.
    pub(super) fn expand_target(&self) -> Option<(&String, &CanvasExpandPreview)> {
        if self.gesture.is_some() || self.inline.is_some() || !self.transform_tool() {
            return None;
        }
        let id = self.selected.as_ref()?;
        let preview = self.presented.as_ref()?.canvas_expand.get(id)?;
        Some((id, preview))
    }

    /// `.screenshot-canvas-expand-action`'s label, as a semibold `text-xs`
    /// layout.
    fn expand_label(&self) -> Option<gfx::Text> {
        let text = self
            .g
            .text(&self.t, Style::Status, EXPAND_CANVAS, 1e4)
            .ok()?;
        let all = DWRITE_TEXT_RANGE {
            startPosition: 0,
            length: EXPAND_CANVAS.encode_utf16().count() as u32,
        };
        unsafe { text.layout.SetFontWeight(DWRITE_FONT_WEIGHT_SEMI_BOLD, all) }.ok()?;
        Some(text)
    }

    /// The action in well DIPs: `padding: s-2 s-4` around the label, centred
    /// `EXPAND_ACTION_INSET` screen pixels outside the largest overflow gap.
    pub(super) fn expand_rect(&self) -> Option<Rect> {
        let (_, preview) = self.expand_target()?;
        let scale = self.zoom_percent()? / 100.;
        let anchor = inset_anchor(
            preview.anchor,
            preview.anchor_edge,
            EXPAND_ACTION_INSET / scale.max(0.01),
        );
        let (x, y) = self.to_well(anchor)?;
        let label = self.expand_label()?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe { label.layout.GetMetrics(&mut metrics) }.ok()?;
        let t = &self.t;
        let w = (metrics.widthIncludingTrailingWhitespace + 2. * t.n("s-4")).ceil();
        let h = (t.n("text-xs") * t.n("leading-tight") + 2. * t.n("s-2")).ceil();
        Some(Rect::new((x - w / 2.).round(), (y - h / 2.).round(), w, h))
    }

    /// Hovering the action arms the ghost (shipping's `expandButtonHover`).
    pub(super) fn expand_armed(&self) -> bool {
        self.hover == Some(Target::Expand) && self.expand_target().is_some()
    }

    /// Show and place the action, and redraw the overlay when its preview or
    /// armed state changed; runs before every frame.
    pub(super) fn place_expand(&mut self) {
        let Some((_, layered)) = self
            .controls
            .iter()
            .find(|(t, _)| *t == Target::Expand)
            .copied()
        else {
            return;
        };
        let rect = self.expand_rect();
        if let Some(r) = rect {
            self.set_rect(layered.base, r);
            let inner = Rect::new(0., 0., r.w, r.h);
            self.set_rect(layered.lit, inner);
            self.set_rect(layered.ring, inner);
        }
        self.set_visible(layered.base, rect.is_some());
        let shown = self
            .expand_target()
            .map(|(_, preview)| (preview.clone(), self.expand_armed()));
        if shown != self.expand_shown {
            if shown.is_none() && self.focus == Some(Target::Expand) {
                self.set_focus(None, false);
            }
            self.expand_shown = shown;
            self.overlay_changed();
        }
    }

    /// Shipping `expandCanvasToFitElement`: one layer edit, one undo step.
    pub(super) fn expand_canvas(&mut self) {
        let Some((id, _)) = self.expand_target() else {
            return;
        };
        let request = Request::Layer {
            id: id.clone(),
            edit: LayerEdit::ExpandCanvas,
        };
        self.submit(request);
    }

    /// The overflow tint over each off-canvas gap and, when armed, the
    /// dashed ghost of the grown canvas with a bloom outside every crossed
    /// edge.
    pub(super) fn draw_expand_hint(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let Some((_, preview)) = self.expand_target() else {
            return Ok(());
        };
        let accent = self.t.color("theme-accent");
        let tint = |a: f32| D2D1_COLOR_F {
            a: accent.a * a,
            ..accent
        };
        for gap in &preview.gaps {
            let Some(r) = self.well_rect(*gap) else {
                continue;
            };
            gfx::fill(dc, r, tint(0.16))?;
            gfx::border(dc, r, 0., 1., tint(0.5))?;
        }
        if !self.expand_armed() {
            return Ok(());
        }
        let Some(ghost) = self.well_rect(preview.rect) else {
            return Ok(());
        };
        let light = Light {
            color: accent,
            bar: SNAP_EDGE_BAR as f32,
            depth: SNAP_BLOOM as f32,
            stops: SNAP_BLOOM_STOPS,
            halo: SNAP_HALO,
        };
        for edge in &preview.edges {
            edge_light(dc, ghost, *edge, &light)?;
        }
        // `.screenshot-canvas-expand-ghost`: `0 0 0 1px .08` and a 1.5 px
        // dashed border at .42, 2 px at .82 on the crossed sides.
        gfx::ring(dc, ghost, 3., 0., 1., tint(0.08))?;
        unsafe {
            let style = self.g.factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES1 {
                    dashStyle: D2D1_DASH_STYLE_CUSTOM,
                    miterLimit: 10.,
                    ..Default::default()
                },
                Some(&[3., 3.]),
            )?;
            let at = |x: f32, y: f32| windows_numerics::Vector2 { X: x, Y: y };
            let (l, t, r, b) = (ghost.left, ghost.top, ghost.right, ghost.bottom);
            for (edge, from, to) in [
                (CanvasEdge::Top, (l, t), (r, t)),
                (CanvasEdge::Right, (r, t), (r, b)),
                (CanvasEdge::Bottom, (r, b), (l, b)),
                (CanvasEdge::Left, (l, b), (l, t)),
            ] {
                let crossed = preview.edges.contains(&edge);
                let width = if crossed { 2. } else { 1.5 };
                // Inside the border box, as CSS draws a border.
                let inset = width / 2.;
                let (dx, dy) = match edge {
                    CanvasEdge::Top => (0., inset),
                    CanvasEdge::Right => (-inset, 0.),
                    CanvasEdge::Bottom => (0., -inset),
                    CanvasEdge::Left => (inset, 0.),
                };
                dc.DrawLine(
                    at(from.0 + dx, from.1 + dy),
                    at(to.0 + dx, to.1 + dy),
                    &gfx::brush(dc, tint(if crossed { 0.82 } else { 0.42 }))?,
                    width,
                    &style,
                );
            }
        }
        Ok(())
    }

    /// `.screenshot-canvas-expand-action`: accent with `shadow-md`, the label
    /// in `theme-accent-ink`; hover and keyboard focus brighten it 8 %.
    pub(super) fn draw_expand_action(
        &self,
        dc: &ID2D1DeviceContext,
        layer: Layer,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let all = rect(0., 0., w, h);
        let radius = t.n("r-sm");
        let accent = t.color("theme-accent");
        match layer {
            Layer::Ring => gfx::ring(dc, all, radius, 2., 4., accent),
            Layer::Base | Layer::Lit => {
                let fill = if layer == Layer::Lit {
                    let up = |c: f32| (c * 1.08).min(1.);
                    D2D1_COLOR_F {
                        r: up(accent.r),
                        g: up(accent.g),
                        b: up(accent.b),
                        a: accent.a,
                    }
                } else {
                    gfx::shadow(dc, all, radius, t.shadow("shadow-md"))?;
                    accent
                };
                gfx::fill_round(dc, all, radius, fill)?;
                let Some(label) = self.expand_label() else {
                    return Ok(());
                };
                let mut metrics = DWRITE_TEXT_METRICS::default();
                unsafe { label.layout.GetMetrics(&mut metrics)? };
                gfx::text(
                    dc,
                    &label,
                    ((w - metrics.widthIncludingTrailingWhitespace) / 2.).round(),
                    ((h - metrics.height) / 2.).round(),
                    t.color("theme-accent-ink"),
                )
            }
        }
    }
}
