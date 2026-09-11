use captures_capture::LogicalRect;

#[derive(Clone, Copy, Debug)]
pub enum Drag {
    New(f64, f64),
    Move(LogicalRect, f64, f64),
    Resize(LogicalRect, usize),
}

pub fn contains(r: LogicalRect, x: f64, y: f64) -> bool {
    x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height
}

pub fn drag_at(r: LogicalRect, x: f64, y: f64) -> Drag {
    if r.width > 0. && r.height > 0. {
        for (i, (cx, cy)) in [
            (r.x, r.y),
            (r.x + r.width, r.y),
            (r.x, r.y + r.height),
            (r.x + r.width, r.y + r.height),
        ]
        .into_iter()
        .enumerate()
        {
            if (cx - x).abs() <= 8. && (cy - y).abs() <= 8. {
                return Drag::Resize(r, i);
            }
        }
        if contains(r, x, y) {
            return Drag::Move(r, x, y);
        }
    }
    Drag::New(x, y)
}

pub fn drag_rect(
    drag: Drag,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    aspect: Option<f64>,
) -> LogicalRect {
    let (x, y) = (x.clamp(0., width), y.clamp(0., height));
    let (sx, sy) = match drag {
        Drag::Move(mut r, sx, sy) => {
            r.x = (r.x + x - sx).clamp(0., (width - r.width).max(0.));
            r.y = (r.y + y - sy).clamp(0., (height - r.height).max(0.));
            return r;
        }
        Drag::New(sx, sy) => (sx, sy),
        Drag::Resize(r, corner) => match corner {
            0 => (r.x + r.width, r.y + r.height),
            1 => (r.x, r.y + r.height),
            2 => (r.x + r.width, r.y),
            _ => (r.x, r.y),
        },
    };
    let (mut dx, mut dy) = (x - sx, y - sy);
    if let Some(ratio) = aspect.filter(|v| v.is_finite() && *v > 0.) {
        let sign_x = if dx < 0. { -1. } else { 1. };
        let sign_y = if dy < 0. { -1. } else { 1. };
        let available_w = if sign_x > 0. { width - sx } else { sx };
        let available_h = if sign_y > 0. { height - sy } else { sy };
        let w = dx
            .abs()
            .max(dy.abs() * ratio)
            .min(available_w)
            .min(available_h * ratio);
        dx = w * sign_x;
        dy = w / ratio * sign_y;
    }
    LogicalRect {
        x: sx,
        y: sy,
        width: dx,
        height: dy,
    }
    .normalized()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reverse_drag_and_aspect_stay_inside_display() {
        let r = drag_rect(Drag::New(200., 180.), -40., 40., 800., 600., Some(16. / 9.));
        assert_eq!((r.x, r.width), (0., 200.));
        assert_eq!(r.height, 112.5);
        assert_eq!(r.y, 67.5);
    }
    #[test]
    fn moving_preserves_dimensions_and_clamps_both_axes() {
        let initial = LogicalRect {
            x: 40.,
            y: 90.,
            width: 170.,
            height: 85.,
        };
        let r = drag_rect(Drag::Move(initial, 70., 100.), 800., -20., 600., 400., None);
        assert_eq!(
            r,
            LogicalRect {
                x: 430.,
                y: 0.,
                width: 170.,
                height: 85.
            }
        );
    }
    #[test]
    fn corner_resize_anchors_opposite_corner() {
        let r = LogicalRect {
            x: 50.,
            y: 90.,
            width: 170.,
            height: 85.,
        };
        let out = drag_rect(drag_at(r, 219., 91.), 310., 40., 600., 400., None);
        assert_eq!(
            out,
            LogicalRect {
                x: 50.,
                y: 40.,
                width: 260.,
                height: 135.
            }
        );
    }
}
