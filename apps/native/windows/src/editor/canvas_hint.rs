//! Shipping's canvas size hints over the page: the Trim edges hover preview
//! (`.screenshot-canvas-trim-hint`), shown while Trim edges is hovered or
//! focused. The shared `Document::trim_preview` owns the geometry and the
//! `TRIM_*` constants the look, as for GTK; the breathing and particle
//! animations are omitted, as GTK omits them.
use captures_app::editor::{Point, Rect as DocRect};
use captures_app::editor_canvas::{
    CanvasEdge, CanvasTrimPreview, TRIM_BLOOM, TRIM_BLOOM_STOPS, TRIM_EDGE_BAR, TRIM_KEEP_ALPHA,
    TRIM_KEEP_GLOW_ALPHA, TRIM_KEEP_RADIUS, TRIM_KEEP_WIDTH, TRIM_REGION_ALPHA, TRIM_RGB,
};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::core::*;

use super::import::hint_bloom;
use super::{Editor, Target};
use crate::gfx::{self, rect};

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
