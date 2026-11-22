//! Shipping's rotate handle on the selected layer: an accent grip 28 px
//! outside the selection's top edge (or the next spot that fits the canvas),
//! drawn like `drawShapeRotationHandle`, that turns the layer about its
//! centre with Shift snapping by the window's rotation snap. The shared
//! `rotation_handle` and `preview_rotation` own the geometry, as for GTK;
//! the worker previews the turn and `LayerEdit::Rotate` commits it.
use captures_app::editor::{
    Element, LayerEdit, Point, RotationHandle, preview_rotation, rotation_angle, rotation_handle,
};
use captures_app::editor_chrome::SHAPE_TOOLS;
use captures_app::editor_session::Request;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::core::*;
use windows_numerics::Vector2;

use super::{Editor, Gesture};
use crate::gfx;

impl Editor {
    /// `toolShowsTransformChrome`: Select, or a shape tool still editing the
    /// shape it placed.
    pub(super) fn transform_tool(&self) -> bool {
        self.tool == "v" || self.tool == "a" || SHAPE_TOOLS.iter().any(|s| s.key == self.tool)
    }

    /// The selected layer, when it shows the rotate handle: unlocked and
    /// visible, with a transform tool, no label open and no other drag.
    fn rotate_layer(&self) -> Option<&Element> {
        if self.inline.is_some() || !self.transform_tool() {
            return None;
        }
        if self
            .gesture
            .as_ref()
            .is_some_and(|g| !matches!(g, Gesture::Rotate { .. }))
        {
            return None;
        }
        let id = self.selected.as_ref()?;
        let element = self
            .presented
            .as_ref()?
            .document
            .elements
            .iter()
            .find(|e| e.base().id == *id)?;
        (!element.base().locked && element.base().visible).then_some(element)
    }

    /// The grip for the selection as shown, in document points.
    pub(super) fn rotate_grip(&self) -> Option<RotationHandle> {
        let element = self.rotate_layer()?;
        let outline = self.selection_outline()?;
        let (width, height) = self.document_size()?;
        let radians = match &self.gesture {
            Some(Gesture::Rotate { .. }) => self.rotation_now().unwrap_or(0.),
            _ => element.base().rotation(),
        };
        rotation_handle(
            outline,
            radians,
            self.zoom_percent().unwrap_or(100.) / 100.,
            width,
            height,
        )
    }

    /// A press on the grip: the layer, its outline and rotation then.
    pub(super) fn rotate_hit(&self, point: Point) -> Option<(String, [Point; 4], f64)> {
        let grip = self.rotate_grip()?;
        if (point.x - grip.handle.x).hypot(point.y - grip.handle.y) > grip.hit_radius {
            return None;
        }
        let element = self.rotate_layer()?;
        Some((
            element.base().id.clone(),
            self.selection_outline()?,
            element.base().rotation(),
        ))
    }

    /// The drag's rotation so far, Shift-snapped.
    fn rotation_now(&self) -> Option<f64> {
        let Some(Gesture::Rotate {
            outline,
            initial,
            start,
            end,
            ..
        }) = &self.gesture
        else {
            return None;
        };
        let snap = self.shift_down().then_some(self.rotation_snap);
        let preview = preview_rotation(*outline, *initial, *start, *end, snap)?;
        rotation_angle(preview.radians, None)
    }

    /// The request a rotate drag previews and commits, `None` when it
    /// has not turned.
    pub(super) fn rotate_request(&self) -> Option<Request> {
        let Some(Gesture::Rotate { id, initial, .. }) = &self.gesture else {
            return None;
        };
        let radians = self.rotation_now()?;
        ((radians - initial).abs() > 1e-9).then(|| Request::Layer {
            id: id.clone(),
            edit: LayerEdit::Rotate { radians },
        })
    }

    /// `drawShapeRotationHandle`: a stem from the edge, a translucent accent
    /// disc with an accent rim, and a compact rotate glyph, in well DIPs.
    pub(super) fn draw_rotate_handle(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let Some(grip) = self.rotate_grip() else {
            return Ok(());
        };
        let (Some(anchor), Some(handle)) = (self.to_well(grip.anchor), self.to_well(grip.handle))
        else {
            return Ok(());
        };
        let accent = self.t.color("theme-accent");
        let brush = gfx::brush(dc, accent)?;
        let disc = gfx::brush(
            dc,
            D2D1_COLOR_F {
                a: accent.a * 0.22,
                ..accent
            },
        )?;
        let at = |x: f32, y: f32| Vector2 { X: x, Y: y };
        let circle = |r: f32| D2D1_ELLIPSE {
            point: at(handle.0, handle.1),
            radiusX: r,
            radiusY: r,
        };
        let round = D2D1_STROKE_STYLE_PROPERTIES1 {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            ..Default::default()
        };
        unsafe {
            let style = self.g.factory.CreateStrokeStyle(&round, None)?;
            dc.FillEllipse(&circle(8.), &disc);
            dc.DrawLine(
                at(anchor.0, anchor.1),
                at(handle.0, handle.1),
                &brush,
                1.6,
                None,
            );
            dc.DrawEllipse(&circle(8.), &brush, 1.6, None);
            // The glyph: an arc from 0.35π round to 1.75π, and its arrowhead.
            let r = 3.15;
            let point = |a: f32| at(handle.0 + r * a.cos(), handle.1 + r * a.sin());
            let pi = std::f32::consts::PI;
            let arc = self.g.factory.CreatePathGeometry()?;
            let sink = arc.Open()?;
            sink.BeginFigure(point(pi * 0.35), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: point(pi * 1.75),
                size: D2D_SIZE_F {
                    width: r,
                    height: r,
                },
                rotationAngle: 0.,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_LARGE,
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            dc.DrawGeometry(&arc, &brush, 1.35, &style);
            let tip = point(pi * 1.75);
            let a = 2.15;
            let head = self.g.factory.CreatePathGeometry()?;
            let sink = head.Open()?;
            sink.BeginFigure(tip, D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(at(tip.X - a * 0.85, tip.Y - a * 0.15));
            sink.AddLine(at(tip.X - a * 0.1, tip.Y + a * 0.9));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            dc.FillGeometry(&head, &brush, None);
        }
        Ok(())
    }
}
