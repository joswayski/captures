//! The Eraser tool (shipping `remove-bg`): Wand removes a color with
//! `RemoveImageBackground`; Erase and Restore paint with the shared round
//! brush. A stroke previews on the worker (`PaintImageBackground` through
//! `preview_drawing_with_geometry`, one request in flight) and commits once
//! on release, so one stroke is one undo step. The brush ring is drawn in
//! the overlay at `brush_cursor` metrics and follows the pointer, and
//! Properties shows shipping's `BrushPreview` dab above Size and Softness.
use captures_app::editor::Point;
use captures_app::editor_chrome::{brush_cursor, draw_preview};
use captures_app::editor_image_background::{BrushMode, wand_target};
use captures_app::editor_session::Request;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::UI::WindowsAndMessaging::{IDC_CROSS, IDC_NO, IDC_SIZEALL};
use windows::core::*;
use windows_numerics::Vector2;

use super::props::EraserMode;
use super::{Editor, Gesture};
use crate::gfx::{self, rect};
use crate::scene::Rect;

fn rgba([r, g, b, a]: [f64; 4]) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32,
        g: g as f32,
        b: b as f32,
        a: a as f32,
    }
}

impl Editor {
    /// The brush mode while the Eraser paints (not Wand).
    pub(super) fn brush_mode(&self) -> Option<BrushMode> {
        match (self.tool, self.eraser.mode) {
            ("b", EraserMode::Brush(mode)) => Some(mode),
            _ => None,
        }
    }

    fn over_image(&self, point: Point) -> bool {
        self.presented
            .as_ref()
            .is_some_and(|p| wand_target(&p.document, point).is_some())
    }

    /// An Eraser press: a Wand removal now, or the start of a stroke.
    pub(super) fn begin_eraser(&mut self, point: Point) {
        if !self.over_image(point) {
            return;
        }
        if self.brush_mode().is_some() {
            self.gesture = Some(Gesture::Brush(vec![point]));
            self.preview_gesture();
            self.overlay_changed();
            return;
        }
        self.notice = None;
        self.submit(Request::RemoveImageBackground {
            point,
            tolerance: self.eraser.tolerance,
            contiguous: self.eraser.contiguous,
        });
    }

    pub(super) fn brush_request(&self, points: Vec<Point>) -> Option<Request> {
        Some(Request::PaintImageBackground {
            points,
            size: self.eraser.size,
            softness: self.eraser.softness,
            mode: self.brush_mode()?,
        })
    }

    /// The ring's centre and radius, well-relative, while it shows.
    pub(super) fn brush_ring(&self) -> Option<(f32, f32, f32)> {
        self.brush_mode()?;
        let (x, y) = self.pointer?;
        if !self.l.well.contains(x, y) || self.hit(x, y).is_some() {
            return None;
        }
        let stroking = matches!(self.gesture, Some(Gesture::Brush(_)));
        let over = self.to_document(x, y).is_some_and(|p| self.over_image(p));
        if brush_cursor::hover(over, stroking, false) != brush_cursor::Hover::Ring {
            return None;
        }
        let scale = self.zoom_percent().unwrap_or(100.) / 100.;
        let d = brush_cursor::screen_diameter(self.eraser.size, scale) as f32;
        Some((x - self.l.well.x, y - self.l.well.y, d / 2.))
    }

    /// The system cursor over the canvas with the Eraser; `None` hides it
    /// under the ring.
    pub(super) fn eraser_cursor(&self, x: f32, y: f32) -> Option<PCWSTR> {
        let panning = matches!(self.gesture, Some(Gesture::Pan { .. }));
        if self.brush_mode().is_none() {
            return Some(if panning { IDC_SIZEALL } else { IDC_CROSS });
        }
        let stroking = matches!(self.gesture, Some(Gesture::Brush(_)));
        let over = self.to_document(x, y).is_some_and(|p| self.over_image(p));
        match brush_cursor::hover(over, stroking, panning) {
            brush_cursor::Hover::Ring => None,
            brush_cursor::Hover::NotAllowed => Some(IDC_NO),
            brush_cursor::Hover::Pan => Some(IDC_SIZEALL),
        }
    }

    /// Shipping's brush ring: a faint fill, a dark halo, the white ring
    /// (dashed for Restore) and a dark inner hairline.
    pub(super) fn draw_brush_ring(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let (Some((x, y, r)), Some(mode)) = (self.brush_ring(), self.brush_mode()) else {
            return Ok(());
        };
        let ring = brush_cursor::RING_WIDTH as f32;
        let halo = brush_cursor::HALO_WIDTH as f32;
        let inset = brush_cursor::INSET_WIDTH as f32;
        let ellipse = |radius: f32| D2D1_ELLIPSE {
            point: Vector2 { X: x, Y: y },
            radiusX: radius.max(0.5),
            radiusY: radius.max(0.5),
        };
        let fill = match mode {
            BrushMode::Erase => rgba(brush_cursor::ERASE_FILL_RGBA),
            BrushMode::Restore => self.t.color_alpha(
                "theme-accent",
                brush_cursor::RESTORE_FILL_ACCENT_ALPHA as f32,
            ),
        };
        unsafe {
            dc.FillEllipse(&ellipse(r), &gfx::brush(dc, fill)?);
            dc.DrawEllipse(
                &ellipse(r + ring / 2. + halo / 2.),
                &gfx::brush(dc, rgba(brush_cursor::HALO_RGBA))?,
                halo,
                None,
            );
            let dashed = if mode == BrushMode::Restore {
                // Dash lengths are in stroke widths.
                let dashes = brush_cursor::RESTORE_DASH.map(|d| d as f32 / ring);
                Some(ID2D1StrokeStyle::from(self.g.factory.CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES1 {
                        dashStyle: D2D1_DASH_STYLE_CUSTOM,
                        ..Default::default()
                    },
                    Some(&dashes),
                )?))
            } else {
                None
            };
            dc.DrawEllipse(
                &ellipse(r),
                &gfx::brush(dc, rgba(brush_cursor::RING_RGBA))?,
                ring,
                dashed.as_ref(),
            );
            dc.DrawEllipse(
                &ellipse(r - ring / 2. - inset / 2.),
                &gfx::brush(dc, rgba(brush_cursor::INSET_RGBA))?,
                inset,
                None,
            );
        }
        Ok(())
    }

    /// Shipping `BrushPreview` (`.screenshot-draw-preview-brush`): an 88 px
    /// checkerboard card with the `editor-border` rim, and the brush dab in
    /// `--solid`, opaque to the softness stop and clear at its rim, sized
    /// from Size in the `0 0 160 72` view box.
    pub(super) fn draw_brush_preview(&self, dc: &ID2D1DeviceContext, r: Rect) -> Result<()> {
        let t = &self.t;
        let frame = rect(r.x, r.y, r.x + r.w, r.y + r.h);
        let radius = t.n("r-md");
        let preview = draw_preview::brush(self.eraser.size, self.eraser.softness);
        unsafe {
            let shape = self
                .g
                .factory
                .CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                    rect: frame,
                    radiusX: radius,
                    radiusY: radius,
                })?;
            dc.PushLayer(
                &D2D1_LAYER_PARAMETERS1 {
                    contentBounds: frame,
                    geometricMask: std::mem::ManuallyDrop::new(Some(shape.cast()?)),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    maskTransform: windows_numerics::Matrix3x2::identity(),
                    opacity: 1.,
                    ..Default::default()
                },
                None,
            );
            // `background-size: 16px` of 45° wedges: 8 px squares.
            gfx::fill(dc, frame, t.color("canvas-checker-b"))?;
            let a = gfx::brush(dc, t.color("canvas-checker-a"))?;
            let cell = draw_preview::CHECKER as f32;
            let (columns, rows) = ((r.w / cell).ceil() as i32, (r.h / cell).ceil() as i32);
            for row in 0..rows {
                for column in (row % 2..columns).step_by(2) {
                    let (x, y) = (r.x + column as f32 * cell, r.y + row as f32 * cell);
                    dc.FillRectangle(&rect(x, y, x + cell, y + cell), &a);
                }
            }
            if let Some(dab) = preview.brush {
                let (w, h) = (f64::from(r.w) - 2., f64::from(r.h) - 2.);
                let (scale, left, top) = draw_preview::fit(w, h);
                let x = r.x + 1. + (left + dab.center.x * scale) as f32;
                let y = r.y + 1. + (top + dab.center.y * scale) as f32;
                let radius = (dab.radius * scale) as f32;
                let solid = t.color("solid");
                let stop = |position: f32, a: f32| D2D1_GRADIENT_STOP {
                    position,
                    color: D2D1_COLOR_F {
                        a: solid.a * a,
                        ..solid
                    },
                };
                let stops = [stop(0., 1.), stop(dab.hard_stop as f32, 1.), stop(1., 0.)];
                let target: &ID2D1RenderTarget = dc;
                let collection = target.CreateGradientStopCollection(
                    &stops,
                    D2D1_GAMMA_2_2,
                    D2D1_EXTEND_MODE_CLAMP,
                )?;
                let centre = Vector2 { X: x, Y: y };
                let fill = target.CreateRadialGradientBrush(
                    &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                        center: centre,
                        gradientOriginOffset: Vector2::default(),
                        radiusX: radius,
                        radiusY: radius,
                    },
                    None,
                    &collection,
                )?;
                dc.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: centre,
                        radiusX: radius,
                        radiusY: radius,
                    },
                    &fill,
                );
            }
            dc.PopLayer();
        }
        gfx::border(dc, frame, radius, 1., t.color("border-subtle"))
    }
}
