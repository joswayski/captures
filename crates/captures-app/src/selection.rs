//! Region interaction geometry in display-local logical coordinates.
//!
//! Ports the shipping `ui/src/lib/selection.ts` rules, including its asymmetric
//! minimum-size/clamping behavior. Hosts supply finite points, positive bounds,
//! and an initial rectangle inside those bounds. Keep the drag's initial state
//! unchanged when updating modifiers; otherwise Shift press/release accumulates
//! rounding or locks in an intermediate square. No window or renderer state here.

pub use captures_capture::LogicalRect as Rect;
use serde::{Deserialize, Serialize};

#[repr(C)]
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Bounds {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DragMode {
    Create,
    Move,
    Nw,
    Ne,
    Sw,
    Se,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DragOptions {
    pub minimum_size: f64,
    pub aspect_ratio: Option<f64>,
    pub force_square: bool,
}
impl Default for DragOptions {
    fn default() -> Self {
        Self {
            minimum_size: 16.,
            aspect_ratio: None,
            force_square: false,
        }
    }
}

pub fn effective_aspect(aspect: Option<f64>, square: bool) -> Option<f64> {
    if square {
        Some(1.)
    } else {
        aspect.filter(|a| a.is_finite() && *a > 0.)
    }
}

pub fn capturable(rect: Option<Rect>) -> bool {
    rect.is_some_and(|r| r.width >= 2. && r.height >= 2.)
}

// Match JS clamp when the available interval is smaller than the requested min.
fn clamp(value: f64, minimum: f64, maximum: f64) -> f64 {
    value.max(minimum).min(maximum.max(minimum))
}

fn rect_between(start: Point, end: Point) -> Rect {
    Rect {
        x: start.x.min(end.x),
        y: start.y.min(end.y),
        width: (end.x - start.x).abs(),
        height: (end.y - start.y).abs(),
    }
}

pub fn drag(
    mode: DragMode,
    origin: Point,
    current: Point,
    initial: Rect,
    bounds: Bounds,
    options: DragOptions,
) -> Rect {
    let aspect = effective_aspect(options.aspect_ratio, options.force_square);
    if matches!(mode, DragMode::Create) {
        let start = Point {
            x: clamp(origin.x, 0., bounds.width),
            y: clamp(origin.y, 0., bounds.height),
        };
        let mut end = Point {
            x: clamp(current.x, 0., bounds.width),
            y: clamp(current.y, 0., bounds.height),
        };
        if let Some(aspect) = aspect {
            let sign_x = if end.x < start.x { -1. } else { 1. };
            let sign_y = if end.y < start.y { -1. } else { 1. };
            let mut width = (end.x - start.x).abs();
            let mut height = (end.y - start.y).abs();
            if height != 0. && width / height <= aspect {
                width = height * aspect;
            }
            width = width.min(if sign_x > 0. {
                bounds.width - start.x
            } else {
                start.x
            });
            height = width / aspect;
            let max_height = if sign_y > 0. {
                bounds.height - start.y
            } else {
                start.y
            };
            if height > max_height {
                height = max_height;
                width = height * aspect;
            }
            end = Point {
                x: start.x + width * sign_x,
                y: start.y + height * sign_y,
            };
        }
        return rect_between(start, end);
    }
    let dx = current.x - origin.x;
    let dy = current.y - origin.y;
    if matches!(mode, DragMode::Move) {
        return Rect {
            x: clamp(initial.x + dx, 0., (bounds.width - initial.width).max(0.)),
            y: clamp(initial.y + dy, 0., (bounds.height - initial.height).max(0.)),
            ..initial
        };
    }
    let west = matches!(mode, DragMode::Nw | DragMode::Sw);
    let north = matches!(mode, DragMode::Nw | DragMode::Ne);
    if let Some(aspect) = aspect {
        return resize_aspect(
            west,
            north,
            current,
            initial,
            bounds,
            aspect,
            options.minimum_size,
        );
    }
    let (mut left, mut top) = (initial.x, initial.y);
    let (mut right, mut bottom) = (left + initial.width, top + initial.height);
    if west {
        left = clamp(left + dx, 0., right - options.minimum_size);
    } else {
        right = clamp(right + dx, left + options.minimum_size, bounds.width);
    }
    if north {
        top = clamp(top + dy, 0., bottom - options.minimum_size);
    } else {
        bottom = clamp(bottom + dy, top + options.minimum_size, bounds.height);
    }
    Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

fn resize_aspect(
    west: bool,
    north: bool,
    current: Point,
    initial: Rect,
    bounds: Bounds,
    aspect: f64,
    minimum_size: f64,
) -> Rect {
    let anchor = Point {
        x: if west {
            initial.x + initial.width
        } else {
            initial.x
        },
        y: if north {
            initial.y + initial.height
        } else {
            initial.y
        },
    };
    let pointer = Point {
        x: clamp(current.x, 0., bounds.width),
        y: clamp(current.y, 0., bounds.height),
    };
    // A locked corner may cross the opposite anchor; a freeform corner may not.
    let positive_x = pointer.x >= anchor.x;
    let positive_y = pointer.y >= anchor.y;
    let mut width = (pointer.x - anchor.x).abs().max(1e-6);
    let mut height = (pointer.y - anchor.y).abs().max(1e-6);
    if width / height > aspect {
        height = width / aspect;
    } else {
        width = height * aspect;
    }
    let max_width = if positive_x {
        bounds.width - anchor.x
    } else {
        anchor.x
    };
    let max_height = if positive_y {
        bounds.height - anchor.y
    } else {
        anchor.y
    };
    if width > max_width {
        width = max_width;
        height = width / aspect;
    }
    if height > max_height {
        height = max_height;
        width = height * aspect;
    }
    let min = minimum_size.max(1.);
    if (width < min || height < min)
        && max_width >= min
        && max_height >= min / aspect
        && max_height >= min
        && max_width >= min * aspect
    {
        if aspect >= 1. {
            width = width.min(max_width).max(min);
            height = width / aspect;
            if height < min || height > max_height {
                height = height.min(max_height).max(min);
                width = height * aspect;
            }
        } else {
            height = height.min(max_height).max(min);
            width = height * aspect;
            if width < min || width > max_width {
                width = width.min(max_width).max(min);
            }
        }
    }
    width = width.min(max_width).max(0.);
    height = width / aspect;
    if height > max_height {
        height = max_height.max(0.);
        width = height * aspect;
    }
    Rect {
        x: if positive_x {
            anchor.x
        } else {
            anchor.x - width
        },
        y: if positive_y {
            anchor.y
        } else {
            anchor.y - height
        },
        width,
        height,
    }
}

/// Refit a settled selection around its center when changing the aspect preset.
pub fn constrain(
    rect: Rect,
    aspect: Option<f64>,
    bounds: Option<Bounds>,
    minimum_size: f64,
) -> Rect {
    let Some(aspect) = effective_aspect(aspect, false) else {
        return rect;
    };
    if rect.width <= 0. || rect.height <= 0. {
        return rect;
    }
    let (mut width, mut height) = if rect.width / rect.height > aspect {
        (rect.height * aspect, rect.height)
    } else {
        (rect.width, rect.width / aspect)
    };
    let min = minimum_size.max(1.);
    if width < min || height < min {
        if aspect >= 1. {
            width = width.max(min);
            height = width / aspect;
        } else {
            height = height.max(min);
            width = height * aspect;
        }
    }
    let mut x = rect.x + (rect.width - width) / 2.;
    let mut y = rect.y + (rect.height - height) / 2.;
    if let Some(bounds) = bounds.filter(|b| b.width > 0. && b.height > 0.) {
        if width > bounds.width {
            width = bounds.width;
            height = width / aspect;
        }
        if height > bounds.height {
            height = bounds.height;
            width = height * aspect;
        }
        x = clamp(x, 0., (bounds.width - width).max(0.));
        y = clamp(y, 0., (bounds.height - height).max(0.));
    }
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Deserialize)]
    struct Case {
        mode: DragMode,
        origin: Point,
        current: Point,
        initial: Rect,
        bounds: Bounds,
        options: DragOptions,
        expected: Rect,
        constrained: Rect,
    }
    #[test]
    fn matches_shipping_drag_and_settled_aspect_vectors() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../tests/selection-golden.json")).unwrap();
        assert!(cases.len() >= 100);
        for (index, case) in cases.into_iter().enumerate() {
            for (actual, expected) in [
                (
                    drag(
                        case.mode,
                        case.origin,
                        case.current,
                        case.initial,
                        case.bounds,
                        case.options,
                    ),
                    case.expected,
                ),
                (
                    constrain(
                        case.initial,
                        case.options.aspect_ratio,
                        Some(case.bounds),
                        case.options.minimum_size,
                    ),
                    case.constrained,
                ),
            ] {
                for (actual, expected) in [actual.x, actual.y, actual.width, actual.height]
                    .into_iter()
                    .zip([expected.x, expected.y, expected.width, expected.height])
                {
                    assert!(
                        (actual - expected).abs() < 1e-9,
                        "case {index}: {actual} != {expected}"
                    );
                }
            }
        }
    }
    #[test]
    fn minimum_capture_boundary_and_shift_release_are_not_sticky() {
        assert!(!capturable(None));
        assert!(!capturable(Some(Rect {
            width: 2.,
            height: 1.999,
            ..Rect::default()
        })));
        assert!(capturable(Some(Rect {
            width: 2.,
            height: 2.,
            ..Rect::default()
        })));
        let initial = Rect::default();
        let origin = Point { x: 100., y: 80. };
        let current = Point { x: 300., y: 180. };
        let bounds = Bounds {
            width: 800.,
            height: 600.,
        };
        let square = drag(
            DragMode::Create,
            origin,
            current,
            initial,
            bounds,
            DragOptions {
                force_square: true,
                aspect_ratio: Some(16. / 9.),
                ..Default::default()
            },
        );
        assert_eq!(
            square,
            Rect {
                x: 100.,
                y: 80.,
                width: 200.,
                height: 200.
            }
        );
        let free = drag(
            DragMode::Create,
            origin,
            current,
            initial,
            bounds,
            DragOptions::default(),
        );
        assert_eq!(
            free,
            Rect {
                x: 100.,
                y: 80.,
                width: 200.,
                height: 100.
            }
        );
        for invalid in [0., -1., f64::NAN, f64::INFINITY] {
            assert_eq!(effective_aspect(Some(invalid), false), None);
            assert_eq!(effective_aspect(Some(invalid), true), Some(1.));
        }
    }
}
