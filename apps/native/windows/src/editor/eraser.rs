//! The Eraser tool (shipping `remove-bg`): Wand removes a color with
//! `RemoveImageBackground`; Erase and Restore paint with the shared round
//! brush. A stroke previews on the worker (`PaintImageBackground` through
//! `preview_drawing_with_geometry`, one request in flight) and commits once
//! on release, so one stroke is one undo step. The brush ring is drawn in
//! the overlay at `brush_cursor` metrics and follows the pointer.
use captures_app::editor::Point;
use captures_app::editor_chrome::brush_cursor;
use captures_app::editor_image_background::{BrushMode, wand_target};
use captures_app::editor_session::Request;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::UI::WindowsAndMessaging::{IDC_CROSS, IDC_NO, IDC_SIZEALL};
use windows::core::*;
use windows_numerics::Vector2;

use super::props::EraserMode;
use super::{Editor, Gesture};
use crate::gfx;

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
}
