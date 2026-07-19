use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl LogicalRect {
    #[must_use]
    pub fn normalized(self) -> Self {
        let x = self.x.min(self.x + self.width);
        let y = self.y.min(self.y + self.height);
        let right = self.x.max(self.x + self.width);
        let bottom = self.y.max(self.y + self.height);

        Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }

    #[must_use]
    pub fn to_physical(self, scale_factor: f64, max_width: u32, max_height: u32) -> PhysicalRect {
        let normalized = self.normalized();
        let left = scaled_coordinate(normalized.x, scale_factor);
        let top = scaled_coordinate(normalized.y, scale_factor);
        let right = scaled_coordinate(normalized.x + normalized.width, scale_factor);
        let bottom = scaled_coordinate(normalized.y + normalized.height, scale_factor);

        let left = left.min(max_width);
        let top = top.min(max_height);
        let right = right.min(max_width);
        let bottom = bottom.min(max_height);

        PhysicalRect {
            x: left,
            y: top,
            width: right.saturating_sub(left),
            height: bottom.saturating_sub(top),
        }
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scaled_coordinate(value: f64, scale_factor: f64) -> u32 {
    (value * scale_factor)
        .round()
        .clamp(0.0, f64::from(u32::MAX)) as u32
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PhysicalRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::{LogicalRect, PhysicalRect};

    #[test]
    fn normalizes_reverse_drag_and_applies_scale() {
        let rect = LogicalRect {
            x: 300.0,
            y: 200.0,
            width: -100.0,
            height: -50.0,
        };

        assert_eq!(
            rect.to_physical(2.0, 1000, 800),
            PhysicalRect {
                x: 400,
                y: 300,
                width: 200,
                height: 100,
            }
        );
    }

    #[test]
    fn clamps_to_display_bounds() {
        let rect = LogicalRect {
            x: -20.0,
            y: -10.0,
            width: 700.0,
            height: 500.0,
        };

        assert_eq!(
            rect.to_physical(1.0, 640, 480),
            PhysicalRect {
                x: 0,
                y: 0,
                width: 640,
                height: 480,
            }
        );
    }
}
