//! Shipping's line/arrow grips on the selected stroke
//! (`hitTestSelectedAnnotation`, `drawEditorOverlays`): square endpoint
//! grips, round control dots (with a dashed stem for a single bend), or three
//! starter dots while the stroke is straight. Dragging one previews the
//! reshaped stroke on the worker and commits `CurveEdit::Move`; a
//! double-click removes a control dot or adds one on the path. The shared
//! `editor_canvas` owns the geometry and hit testing, as for GTK and AppKit.
//! A shape tool also moves the selected shape it drew from its body.
use captures_app::editor::{Element, LayerEdit, Point, ShapeElement};
use captures_app::editor_canvas::{self, CurveEdit, CurveHandle};
use captures_app::editor_chrome::SHAPE_TOOLS;
use captures_app::editor_session::Request;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use super::{Editor, Gesture};
use crate::gfx::{self, rect};

/// Shipping's pointer slop for grips: 10 screen pixels.
pub(super) const HIT_PX: f64 = 10.;

impl Editor {
    /// The selected, unlocked and visible line or arrow under a transform
    /// tool, with no label open.
    pub(super) fn selected_curve(&self) -> Option<&ShapeElement> {
        if self.inline.is_some() || !self.transform_tool() {
            return None;
        }
        let id = self.selected.as_ref()?;
        self.presented
            .as_ref()?
            .document
            .elements
            .iter()
            .find_map(|element| match element {
                Element::Shape(shape)
                    if shape.base.id == *id
                        && !shape.base.locked
                        && shape.base.visible
                        && editor_canvas::is_curveable(shape) =>
                {
                    Some(shape)
                }
                _ => None,
            })
    }

    fn hit_radius(&self) -> f64 {
        HIT_PX / (self.zoom_percent().unwrap_or(100.) / 100.)
    }

    /// The curve dot or endpoint under a document point.
    pub(super) fn curve_hit(&self, point: Point) -> Option<(String, CurveHandle)> {
        let shape = self.selected_curve()?;
        let handle = editor_canvas::hit_test_curve_handle(shape, point, self.hit_radius())?;
        Some((shape.base.id.clone(), handle))
    }

    /// Shipping's double-click on a selected line or arrow: a control dot
    /// goes away, and the path gains a dot there.
    pub(super) fn edit_curve_at(&mut self, point: Point) -> bool {
        let Some(shape) = self.selected_curve() else {
            return false;
        };
        let radius = self.hit_radius();
        let edit = match editor_canvas::hit_test_curve_handle(shape, point, radius) {
            Some(CurveHandle::Control { index }) => CurveEdit::Remove { index },
            Some(_) => return false,
            None => {
                let (_, _, distance) = editor_canvas::closest_point_on_curve(shape, point);
                if distance > editor_canvas::curve_path_hit_radius(shape, radius) {
                    return false;
                }
                CurveEdit::Insert { point }
            }
        };
        let id = shape.base.id.clone();
        self.submit(Request::Layer {
            id,
            edit: LayerEdit::Curve { edit },
        });
        true
    }

    /// Shipping `hitTestSelectedShapeBody`: under a shape tool, a press on
    /// the selected shape of that kind (a line's or arrow's path, another
    /// shape's body) moves it instead of drawing a new one.
    pub(super) fn shape_body_hit(&self, point: Point) -> Option<String> {
        let shape = match self.tool {
            "a" => "arrow",
            key if SHAPE_TOOLS.iter().any(|tool| tool.key == key) => key,
            _ => return None,
        };
        if self.inline.is_some() {
            return None;
        }
        let id = self.selected.as_ref()?;
        let element = self
            .presented
            .as_ref()?
            .document
            .elements
            .iter()
            .find(|element| element.base().id == *id)?;
        // Shipping's 8 px selection slop, widened to the grip's 10.
        let radius = HIT_PX / 2. / (self.zoom_percent().unwrap_or(100.) / 100.);
        editor_canvas::selected_shape_body_hit(element, shape, point, radius)
            .unwrap_or(false)
            .then(|| id.clone())
    }

    /// The request a curve drag previews and commits.
    pub(super) fn curve_request(id: &str, handle: CurveHandle, end: Point) -> Request {
        Request::Layer {
            id: id.to_owned(),
            edit: LayerEdit::Curve {
                edit: CurveEdit::Move { handle, point: end },
            },
        }
    }

    /// The endpoints, control dots and starter dots, in well DIPs, for the
    /// stroke as shown: an idle selection, or the one being reshaped.
    pub(super) fn draw_curve_handles(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        if self.held_outline.is_some() {
            return Ok(());
        }
        let Some(shape) = self.selected_curve() else {
            return Ok(());
        };
        let mut shape = shape.clone();
        // An idle selection, or the stroke as the drag reshapes it.
        let shown = match &self.gesture {
            None => true,
            Some(Gesture::Curve {
                id, handle, end, ..
            }) if *id == shape.base.id => editor_canvas::apply_curve_edit(
                &mut shape,
                CurveEdit::Move {
                    handle: *handle,
                    point: *end,
                },
            )
            .is_ok(),
            Some(_) => false,
        };
        if !shown {
            return Ok(());
        }
        let Some(handles) = editor_canvas::curve_handles(&shape) else {
            return Ok(());
        };
        let (Some(start), Some(end)) = (self.to_well(handles.start), self.to_well(handles.end))
        else {
            return Ok(());
        };
        let map = |points: &[Point]| -> Vec<(f32, f32)> {
            points.iter().filter_map(|p| self.to_well(*p)).collect()
        };
        let (controls, starters) = (map(&handles.controls), map(&handles.starters));
        let accent = self.t.color("theme-accent");
        let alpha = |a: f32| D2D1_COLOR_F {
            a: accent.a * a,
            ..accent
        };
        let white = |a: f32| D2D1_COLOR_F {
            r: 1.,
            g: 1.,
            b: 1.,
            a,
        };
        // A single bend's dashed stem from the chord's midpoint
        // (`setLineDash([3, 3])`: dashes in stroke widths).
        if let [control] = controls.as_slice() {
            let from = ((start.0 + end.0) / 2., (start.1 + end.1) / 2.);
            let style = unsafe {
                self.g.factory.CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES1 {
                        dashStyle: D2D1_DASH_STYLE_CUSTOM,
                        miterLimit: 10.,
                        ..Default::default()
                    },
                    Some(&[3. / 1.15, 3. / 1.15]),
                )?
            };
            let brush = gfx::brush(dc, alpha(0.45))?;
            unsafe {
                dc.DrawLine(
                    Vector2 {
                        X: from.0,
                        Y: from.1,
                    },
                    Vector2 {
                        X: control.0,
                        Y: control.1,
                    },
                    &brush,
                    1.15,
                    &style,
                )
            };
        }
        // Square endpoint grips turned with the layer.
        let angle = shape.base.rotation() as f32;
        let size = 5.5;
        let fill = gfx::brush(dc, alpha(0.88))?;
        let rim = gfx::brush(dc, white(0.9 * 0.88))?;
        for (x, y) in [start, end] {
            unsafe {
                let mut old = Matrix3x2::default();
                dc.GetTransform(&mut old);
                dc.SetTransform(
                    &(Matrix3x2::rotation(angle.to_degrees()) * Matrix3x2::translation(x, y) * old),
                );
                let r = rect(-size / 2., -size / 2., size / 2., size / 2.);
                dc.FillRectangle(&r, &fill);
                dc.DrawRectangle(&r, &rim, 1.1, None);
                dc.SetTransform(&old);
            }
        }
        // Round control and starter dots.
        let dot_fill = gfx::brush(dc, white(0.94 * 0.82))?;
        let dot_rim = gfx::brush(dc, alpha(0.82))?;
        for (x, y) in controls.iter().chain(&starters) {
            let ellipse = D2D1_ELLIPSE {
                point: Vector2 { X: *x, Y: *y },
                radiusX: 4.5,
                radiusY: 4.5,
            };
            unsafe {
                dc.FillEllipse(&ellipse, &dot_fill);
                dc.DrawEllipse(&ellipse, &dot_rim, 1.15, None);
            }
        }
        Ok(())
    }
}
