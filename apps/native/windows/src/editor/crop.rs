//! The Crop tool and the selected image's geometry: shipping's Crop section
//! (Aspect ratio select, the staged crop's read-only Width and Height, Clear
//! and Apply crop) and its canvas overlay, and the image section's Width,
//! Height, X and Y fields.
//!
//! A crop drag only stages a rectangle through the shared `CropDrag`, as
//! shipping's `cropSelection`; Apply crop commits it through `Request::Crop`.
//! Image geometry goes through `LayerEdit::Geometry`, one undo step per
//! change like shipping's `updateSelected`.
use captures_app::editor::{CropDrag, Element, ImageElement, LayerEdit, Point, Rect as DocRect};
use captures_app::editor_layers::{self, geometry};
use captures_app::editor_session::Request;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::*;

use super::props::{Num, Prop};
use super::{Editor, Gesture, Target};
use crate::gfx::{self, Style, rect};

/// Shipping's Aspect ratio options: `(label, width / height)`.
pub const ASPECTS: [(&str, Option<f64>); 5] = [
    ("Free", None),
    ("1 : 1", Some(1.)),
    ("4 : 3", Some(4. / 3.)),
    ("3 : 2", Some(3. / 2.)),
    ("16 : 9", Some(16. / 9.)),
];
pub const ASPECT_LABEL: &str = "Aspect ratio";
pub const CLEAR: &str = "Clear";
pub const APPLY: &str = "Apply crop";
pub const KEEP_HINT: &str = "Hold Shift while dragging to keep this aspect ratio.";
pub const DRAG_HINT: &str = "Drag over the area you want to keep. Start from outside the canvas \
                             to crop to an edge. Hold Shift to lock the current aspect ratio.";

impl Editor {
    /// The selected image layer.
    pub fn selected_image(&self) -> Option<&ImageElement> {
        let id = self.selected.as_deref()?;
        self.presented
            .as_ref()?
            .document
            .elements
            .iter()
            .find_map(|element| match element {
                Element::Image(image) if image.base.id == id => Some(image),
                _ => None,
            })
    }

    /// A number field the user can't change (a locked layer's geometry).
    pub fn num_disabled(&self, num: Num) -> bool {
        num.is_geometry() && self.selected_image().is_none_or(|i| i.base.locked)
    }

    /// Whether a number field shows its steppers.
    pub fn num_steppers(&self, num: Num) -> bool {
        !num.read_only() && !self.num_disabled(num)
    }

    pub(super) fn geometry_value(&self, num: Num) -> f64 {
        if let Some(r) = self.crop {
            match num {
                Num::CropWidth => return r.width,
                Num::CropHeight => return r.height,
                _ => {}
            }
        }
        let Some(image) = self.selected_image() else {
            return 0.;
        };
        match num {
            Num::LayerWidth => image.width.round(),
            Num::LayerHeight => image.height.round(),
            Num::LayerX => image.base.x.round(),
            Num::LayerY => image.base.y.round(),
            _ => 0.,
        }
    }

    /// Width, Height, X or Y of the selected image: shown at once, then sent
    /// as its own undo step.
    pub(super) fn set_geometry(&mut self, num: Num, value: f64) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if self.num_disabled(num) || value == self.geometry_value(num) {
            return;
        }
        let (mut x, mut y, mut width, mut height) = (None, None, None, None);
        match num {
            Num::LayerWidth => width = Some(value),
            Num::LayerHeight => height = Some(value),
            Num::LayerX => x = Some(value),
            Num::LayerY => y = Some(value),
            _ => return,
        }
        if let Some(Element::Image(image)) = self
            .presented
            .as_mut()
            .and_then(|p| p.document.elements.iter_mut().find(|e| e.base().id == id))
        {
            if let Some(w) = width {
                (image.width, image.height) = editor_layers::image_size_at_width(image, w);
            }
            if let Some(h) = height {
                (image.width, image.height) = editor_layers::image_size_at_height(image, h);
            }
            image.base.x = x.unwrap_or(image.base.x);
            image.base.y = y.unwrap_or(image.base.y);
        }
        self.submit(Request::Layer {
            id,
            edit: LayerEdit::Geometry {
                x,
                y,
                width,
                height,
            },
        });
        for num in [Num::LayerWidth, Num::LayerHeight, Num::LayerX, Num::LayerY] {
            self.prop_dirty(Prop::Number(num));
        }
        self.overlay_changed();
    }

    /// The image section's note.
    pub fn geometry_note(&self) -> &'static str {
        if self.selected_image().is_some_and(|i| i.base.locked) {
            geometry::LOCKED
        } else {
            geometry::PROPORTIONAL
        }
    }

    // ---- crop ----

    fn crop_aspect_value(&self) -> Option<f64> {
        ASPECTS.get(self.crop_aspect).and_then(|a| a.1)
    }

    fn crop_bounds(&self) -> Option<DocRect> {
        let (width, height) = self.document_size()?;
        Some(DocRect {
            x: 0.,
            y: 0.,
            width,
            height,
        })
    }

    /// A press with the Crop tool starts a new staged rectangle.
    pub(super) fn begin_crop(&mut self, point: Point) {
        let Some(bounds) = self.crop_bounds() else {
            return;
        };
        self.set_selected(None);
        let drag = CropDrag::new(point, bounds, self.crop_aspect_value(), self.shift_down());
        self.set_crop(Some(drag.rect()));
        self.gesture = Some(Gesture::Crop(drag));
    }

    pub(super) fn update_crop(&mut self, point: Point) {
        let (aspect, shift) = (self.crop_aspect_value(), self.shift_down());
        let Some(Gesture::Crop(drag)) = self.gesture.as_mut() else {
            return;
        };
        let r = drag.update(point, aspect, shift);
        self.set_crop(Some(r));
    }

    /// Stage, change or drop the crop rectangle.
    pub(super) fn set_crop(&mut self, crop: Option<DocRect>) {
        if crop == self.crop {
            return;
        }
        let structure = crop.is_some() != self.crop.is_some();
        self.crop = crop;
        if structure {
            self.props_changed();
        } else {
            for num in [Num::CropWidth, Num::CropHeight] {
                self.prop_dirty(Prop::Number(num));
                self.events
                    .push(super::Event::Value(Target::Prop(Prop::Number(num))));
            }
            self.sync_field();
        }
        self.overlay_changed();
        self.need = true;
    }

    /// Shipping `applyCrop`: crop, then back to Select with nothing selected.
    pub(super) fn apply_crop(&mut self) {
        let Some(rect) = self.crop else {
            return;
        };
        self.set_crop(None);
        self.set_selected(None);
        self.submit(Request::Crop { rect });
        self.select_tool("v");
    }

    pub fn crop_note(&self) -> &'static str {
        if self.crop.is_some() {
            KEEP_HINT
        } else {
            DRAG_HINT
        }
    }

    /// Shipping `drawEditorOverlays`' crop: the dimmed outside, a white
    /// dashed outline and the `W × H` tag, constant in screen size.
    pub(super) fn draw_crop(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let (Some(crop), Some(r)) = (self.crop, self.image_rect()) else {
            return Ok(());
        };
        let (Some(a), Some(b)) = (
            self.to_well(Point {
                x: crop.x,
                y: crop.y,
            }),
            self.to_well(Point {
                x: crop.x + crop.width,
                y: crop.y + crop.height,
            }),
        ) else {
            return Ok(());
        };
        let shade = D2D1_COLOR_F {
            r: 5. / 255.,
            g: 6. / 255.,
            b: 8. / 255.,
            a: 0.64,
        };
        let (l, tp) = (r.x as f32, r.y as f32);
        let (rr, bt) = ((r.x + r.width) as f32, (r.y + r.height) as f32);
        for part in [
            rect(l, tp, rr, a.1),
            rect(l, b.1, rr, bt),
            rect(l, a.1, a.0, b.1),
            rect(b.0, a.1, rr, b.1),
        ] {
            if part.right > part.left && part.bottom > part.top {
                gfx::fill(dc, part, shade)?;
            }
        }
        let white = D2D1_COLOR_F {
            r: 1.,
            g: 1.,
            b: 1.,
            a: 1.,
        };
        unsafe {
            // `setLineDash([8, 5])` at a 2 px line: dashes in stroke widths.
            let style = self.g.factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES1 {
                    dashStyle: D2D1_DASH_STYLE_CUSTOM,
                    miterLimit: 10.,
                    ..Default::default()
                },
                Some(&[4., 2.5]),
            )?;
            dc.DrawRectangle(
                &rect(a.0, a.1, b.0, b.1),
                &gfx::brush(dc, white)?,
                2.,
                &style,
            );
        }
        // `700 12px`, white on `#111216`, 7 px each side, 24 tall, 8 below
        // the top edge.
        let label = format!("{} × {}", crop.width, crop.height);
        let text = self.g.text(&self.t, Style::Status, &label, 1e4)?;
        unsafe {
            let all = DWRITE_TEXT_RANGE {
                startPosition: 0,
                length: label.encode_utf16().count() as u32,
            };
            text.layout.SetFontSize(12., all)?;
            text.layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, all)?;
        }
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { text.layout.GetMetrics(&mut m)? };
        let w = m.widthIncludingTrailingWhitespace + 14.;
        let cx = (a.0 + b.0) / 2.;
        let tag = rect(cx - w / 2., a.1 + 8., cx + w / 2., a.1 + 32.);
        gfx::fill(
            dc,
            tag,
            D2D1_COLOR_F {
                r: 0x11 as f32 / 255.,
                g: 0x12 as f32 / 255.,
                b: 0x16 as f32 / 255.,
                a: 1.,
            },
        )?;
        unsafe {
            dc.DrawTextLayout(
                windows_numerics::Vector2 {
                    X: cx - m.widthIncludingTrailingWhitespace / 2.,
                    Y: a.1 + 20. - m.height / 2.,
                },
                &text.layout,
                &gfx::brush(dc, white)?,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
        Ok(())
    }

    /// `.screenshot-property-actions button` (Clear) and `.primary` (Apply
    /// crop).
    pub(super) fn draw_crop_action(
        &self,
        dc: &ID2D1DeviceContext,
        apply: bool,
        lit: bool,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let radius = t.n("r-md");
        let (style, ink) = if apply {
            let fill = if lit {
                c("theme-accent-hover")
            } else {
                c("theme-accent")
            };
            gfx::fill_round(dc, all, radius, fill)?;
            (Style::Strong, c("theme-accent-ink"))
        } else {
            let (fill, border) = if lit {
                (c("control-hover"), c("border-strong"))
            } else {
                (c("control"), c("control-border"))
            };
            gfx::fill_round(dc, all, radius, fill)?;
            gfx::border(dc, all, radius, 1., border)?;
            (Style::Button, c("text"))
        };
        let label = self
            .g
            .text(t, style, if apply { APPLY } else { CLEAR }, 1e4)?;
        gfx::text(
            dc,
            &label,
            ((w - label.width) / 2.).round(),
            ((h - label.height) / 2.).round(),
            ink,
        )
    }
}
