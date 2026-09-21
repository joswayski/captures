//! Ephemeral screenshot-editor viewport geometry. Never part of a document or draft.
//! Hosts supply their layout's fitted image rect in top-left logical coordinates.

use crate::editor::{Point, Rect};

/// Zero zoom means the host's fitted layout. Manual zoom is percent of image pixels.
/// Shared with the C ABI; hosts may copy this value and update pan for native input.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Viewport {
    pub zoom_percent: f64,
    pub pan_x: f64,
    pub pan_y: f64,
}

impl Viewport {
    pub fn rect(self, fit: Rect, width: f64, height: f64) -> Option<Rect> {
        if ![
            fit.x,
            fit.y,
            fit.width,
            fit.height,
            width,
            height,
            self.zoom_percent,
            self.pan_x,
            self.pan_y,
        ]
        .into_iter()
        .all(f64::is_finite)
            || fit.width <= 0.
            || fit.height <= 0.
            || width <= 0.
            || height <= 0.
            || self.zoom_percent < 0.
        {
            return None;
        }
        let scale = if self.zoom_percent == 0. {
            fit.width / width
        } else {
            self.zoom_percent / 100.
        };
        let rect = Rect {
            x: fit.x + (fit.width - width * scale) / 2. + self.pan_x,
            y: fit.y + (fit.height - height * scale) / 2. + self.pan_y,
            width: width * scale,
            height: height * scale,
        };
        [rect.x, rect.y, rect.width, rect.height]
            .into_iter()
            .all(f64::is_finite)
            .then_some(rect)
    }

    /// Keep the document point under `anchor` stationary. Mirrors Tauri's 5–800%
    /// bounds and tenth-percent rounding; hosts normalize native event units.
    pub fn zoom_at(
        self,
        fit: Rect,
        width: f64,
        height: f64,
        percent: f64,
        anchor: Point,
    ) -> Option<Self> {
        if !percent.is_finite() || !anchor.x.is_finite() || !anchor.y.is_finite() {
            return None;
        }
        let old = self.rect(fit, width, height)?;
        let mut next = Self {
            zoom_percent: (percent.clamp(5., 800.) * 10.).round() / 10.,
            ..self
        };
        let new = next.rect(fit, width, height)?;
        next.pan_x += anchor.x - (new.x + (anchor.x - old.x) / old.width * new.width);
        next.pan_y += anchor.y - (new.y + (anchor.y - old.y) / old.height * new.height);
        next.rect(fit, width, height)?;
        Some(next)
    }

    pub fn recenter(&mut self) {
        self.pan_x = 0.;
        self.pan_y = 0.;
    }
}

/// Tauri's exponential wheel response, after native events are mapped to pixels.
pub fn wheel_zoom_factor(delta_pixels: f64) -> Option<f64> {
    delta_pixels
        .is_finite()
        .then(|| (-delta_pixels.clamp(-240., 240.) * 0.002).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIT: Rect = Rect {
        x: 30.,
        y: 80.,
        width: 400.,
        height: 150.,
    };

    #[test]
    fn default_fit_and_pan_preserve_host_layout_without_document_changes() {
        assert_eq!(Viewport::default().rect(FIT, 800., 300.), Some(FIT));
        let mut view = Viewport {
            pan_x: -17.,
            pan_y: 23.,
            ..Viewport::default()
        };
        assert_eq!(
            view.rect(FIT, 800., 300.),
            Some(Rect {
                x: 13.,
                y: 103.,
                ..FIT
            })
        );
        view.recenter();
        assert_eq!(view, Viewport::default());
    }

    #[test]
    fn anchored_zoom_preserves_asymmetric_point_after_free_pan() {
        let view = Viewport {
            pan_x: 17.,
            pan_y: -23.,
            ..Viewport::default()
        };
        let next = view
            .zoom_at(FIT, 800., 300., 125., Point { x: 147., y: 97. })
            .unwrap();
        // Original top left is (47,57); anchor is image pixel (200,80).
        // At 125% that pixel is (250,100) from the new top left, hence (-103,-3).
        assert_eq!(
            next.rect(FIT, 800., 300.),
            Some(Rect {
                x: -103.,
                y: -3.,
                width: 1000.,
                height: 375.
            })
        );
        let back = next
            .zoom_at(FIT, 800., 300., 50., Point { x: 147., y: 97. })
            .unwrap();
        assert_eq!(back.rect(FIT, 800., 300.), view.rect(FIT, 800., 300.));
    }

    #[test]
    fn zoom_limits_rounding_recenter_and_fit_are_distinct() {
        let zoom = |percent| {
            Viewport::default()
                .zoom_at(FIT, 800., 300., percent, Point { x: 100., y: 120. })
                .unwrap()
        };
        assert_eq!(zoom(4.9).zoom_percent, 5.);
        assert_eq!(zoom(800.1).zoom_percent, 800.);
        assert_eq!(zoom(123.46).zoom_percent, 123.5);
        let mut view = zoom(100.);
        view.recenter();
        assert_eq!(
            view.rect(FIT, 800., 300.),
            Some(Rect {
                x: -170.,
                y: 5.,
                width: 800.,
                height: 300.
            })
        );
        assert_eq!(Viewport::default().rect(FIT, 800., 300.), Some(FIT));
    }

    #[test]
    fn wheel_response_matches_shipping_pixel_delta_policy() {
        assert_eq!(wheel_zoom_factor(0.), Some(1.));
        assert!((wheel_zoom_factor(80.).unwrap() - 0.8521437889662113).abs() < 1e-12);
        assert!((wheel_zoom_factor(-80.).unwrap() - 1.1735108709918103).abs() < 1e-12);
        assert_eq!(wheel_zoom_factor(400.), wheel_zoom_factor(240.));
        assert_eq!(wheel_zoom_factor(-400.), wheel_zoom_factor(-240.));
        assert_eq!(wheel_zoom_factor(f64::NAN), None);
    }

    #[test]
    fn invalid_geometry_and_overflow_never_produce_a_viewport() {
        let view = Viewport::default();
        assert_eq!(view.rect(FIT, 0., 300.), None);
        assert_eq!(view.rect(Rect { width: 0., ..FIT }, 800., 300.), None);
        assert_eq!(
            view.zoom_at(FIT, 800., 300., f64::INFINITY, Point { x: 0., y: 0. }),
            None
        );
        assert_eq!(
            view.zoom_at(FIT, 800., 300., 100., Point { x: f64::NAN, y: 0. }),
            None
        );
        assert_eq!(
            Viewport {
                pan_x: f64::INFINITY,
                ..view
            }
            .rect(FIT, 800., 300.),
            None
        );
    }
}
