use captures_capture::LogicalRect;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn from_points(a: Point, b: Point) -> Self {
        Self {
            x: a.x.min(b.x),
            y: a.y.min(b.y),
            width: (a.x - b.x).abs(),
            height: (a.y - b.y).abs(),
        }
    }

    pub fn contains(self, point: Point) -> bool {
        point.x >= self.x
            && point.y >= self.y
            && point.x <= self.x + self.width
            && point.y <= self.y + self.height
    }

    pub fn inset(self, amount: f32) -> Self {
        Self {
            x: self.x + amount,
            y: self.y + amount,
            width: (self.width - amount * 2.0).max(0.0),
            height: (self.height - amount * 2.0).max(0.0),
        }
    }

    pub fn to_logical(self) -> LogicalRect {
        LogicalRect {
            x: f64::from(self.x),
            y: f64::from(self.y),
            width: f64::from(self.width),
            height: f64::from(self.height),
        }
    }
}

pub fn rounded_contains(point: Point, rect: Rect, radius: f32) -> bool {
    if !rect.contains(point) {
        return false;
    }
    let radius = radius.max(0.0).min(rect.width.min(rect.height) / 2.0);
    let nearest_x = point.x.clamp(rect.x + radius, rect.x + rect.width - radius);
    let nearest_y = point
        .y
        .clamp(rect.y + radius, rect.y + rect.height - radius);
    (point.x - nearest_x).powi(2) + (point.y - nearest_y).powi(2) <= radius.powi(2)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SelectionDrag {
    Create { anchor: Point },
    Move { original: Rect, anchor: Point },
    Resize { original: Rect, corner: usize },
}

pub fn update_selection(drag: SelectionDrag, pointer: Point, bounds: Rect) -> Rect {
    let pointer = Point {
        x: pointer.x.clamp(bounds.x, bounds.x + bounds.width),
        y: pointer.y.clamp(bounds.y, bounds.y + bounds.height),
    };
    match drag {
        SelectionDrag::Create { anchor } => Rect::from_points(anchor, pointer),
        SelectionDrag::Move { original, anchor } => Rect {
            x: (original.x + pointer.x - anchor.x)
                .clamp(bounds.x, bounds.x + bounds.width - original.width),
            y: (original.y + pointer.y - anchor.y)
                .clamp(bounds.y, bounds.y + bounds.height - original.height),
            ..original
        },
        SelectionDrag::Resize { original, corner } => {
            let opposite = match corner {
                0 => Point {
                    x: original.x + original.width,
                    y: original.y + original.height,
                },
                1 => Point {
                    x: original.x,
                    y: original.y + original.height,
                },
                2 => Point {
                    x: original.x + original.width,
                    y: original.y,
                },
                _ => Point {
                    x: original.x,
                    y: original.y,
                },
            };
            let mut result = Rect::from_points(opposite, pointer);
            if result.width < 16.0 {
                result.width = 16.0;
                result.x = if pointer.x < opposite.x {
                    opposite.x - 16.0
                } else {
                    opposite.x
                };
            }
            if result.height < 16.0 {
                result.height = 16.0;
                result.y = if pointer.y < opposite.y {
                    opposite.y - 16.0
                } else {
                    opposite.y
                };
            }
            result
        }
    }
}

pub fn cover(source: (u32, u32), destination: Rect) -> Rect {
    let scale = (destination.width / source.0.max(1) as f32)
        .max(destination.height / source.1.max(1) as f32);
    let width = source.0 as f32 * scale;
    let height = source.1 as f32 * scale;
    Rect {
        x: destination.x + (destination.width - width) / 2.0,
        y: destination.y + (destination.height - height) / 2.0,
        width,
        height,
    }
}

pub fn contain(source: (u32, u32), destination: Rect) -> Rect {
    let scale = (destination.width / source.0.max(1) as f32)
        .min(destination.height / source.1.max(1) as f32);
    let width = source.0 as f32 * scale;
    let height = source.1 as f32 * scale;
    Rect {
        x: destination.x + (destination.width - width) / 2.0,
        y: destination.y + (destination.height - height) / 2.0,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_create_is_normalized() {
        assert_eq!(
            update_selection(
                SelectionDrag::Create {
                    anchor: Point { x: 80.0, y: 70.0 }
                },
                Point { x: 20.0, y: 10.0 },
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: 100.0
                },
            ),
            Rect {
                x: 20.0,
                y: 10.0,
                width: 60.0,
                height: 60.0
            }
        );
    }

    #[test]
    fn move_clamps_both_far_edges_without_resizing() {
        let result = update_selection(
            SelectionDrag::Move {
                original: Rect {
                    x: 10.0,
                    y: 15.0,
                    width: 40.0,
                    height: 30.0,
                },
                anchor: Point { x: 20.0, y: 20.0 },
            },
            Point { x: 200.0, y: -50.0 },
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 80.0,
            },
        );
        assert_eq!(
            result,
            Rect {
                x: 60.0,
                y: 0.0,
                width: 40.0,
                height: 30.0
            }
        );
    }

    #[test]
    fn cover_crops_wide_image_symmetrically() {
        assert_eq!(
            cover(
                (400, 200),
                Rect {
                    x: 10.0,
                    y: 20.0,
                    width: 100.0,
                    height: 100.0
                }
            ),
            Rect {
                x: -40.0,
                y: 20.0,
                width: 200.0,
                height: 100.0
            }
        );
    }

    #[test]
    fn contain_letterboxes_wide_image_without_distortion() {
        assert_eq!(
            contain(
                (400, 200),
                Rect {
                    x: 10.0,
                    y: 20.0,
                    width: 100.0,
                    height: 100.0,
                }
            ),
            Rect {
                x: 10.0,
                y: 45.0,
                width: 100.0,
                height: 50.0,
            }
        );
    }

    #[test]
    fn rounded_hit_test_excludes_only_corner_cutouts() {
        let rect = Rect {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 60.0,
        };
        assert!(!rounded_contains(Point { x: 10.0, y: 20.0 }, rect, 12.0));
        assert!(rounded_contains(Point { x: 16.0, y: 26.0 }, rect, 12.0));
        assert!(rounded_contains(Point { x: 60.0, y: 20.0 }, rect, 12.0));
        assert!(!rounded_contains(Point { x: 111.0, y: 50.0 }, rect, 12.0));
    }
}
