//! Allocation-free geometry ABI for pointer/modifier events. No per-frame JSON.
use captures_app::selection::{self, Bounds, DragMode, DragOptions, Point, Rect};

fn valid(rect: Rect, bounds: Bounds, aspect: f64) -> bool {
    [
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        bounds.width,
        bounds.height,
        aspect,
    ]
    .into_iter()
    .all(f64::is_finite)
        && rect.x >= 0.
        && rect.y >= 0.
        && rect.width >= 0.
        && rect.height >= 0.
        && bounds.width > 0.
        && bounds.height > 0.
        && aspect >= 0.
        && rect.x + rect.width <= bounds.width
        && rect.y + rect.height <= bounds.height
}

/// `mode`: create, move, NW, NE, SW, SE = 0..5. Aspect 0 is freeform.
/// False leaves output unchanged. The shipping resize minimum is 16 logical units.
///
/// # Safety
/// A non-null `output` must be aligned, writable storage for one Rect. Inputs are
/// copied by value and not retained. No allocator or event-loop thread is required.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_selection_drag_v1(
    mode: u32,
    origin: Point,
    current: Point,
    initial: Rect,
    bounds: Bounds,
    aspect: f64,
    force_square: bool,
    output: *mut Rect,
) -> bool {
    let mode = match mode {
        0 => DragMode::Create,
        1 => DragMode::Move,
        2 => DragMode::Nw,
        3 => DragMode::Ne,
        4 => DragMode::Sw,
        5 => DragMode::Se,
        _ => return false,
    };
    if output.is_null()
        || !valid(initial, bounds, aspect)
        || ![origin.x, origin.y, current.x, current.y]
            .into_iter()
            .all(f64::is_finite)
    {
        return false;
    }
    let rect = selection::drag(
        mode,
        origin,
        current,
        initial,
        bounds,
        DragOptions {
            aspect_ratio: (aspect > 0.).then_some(aspect),
            force_square,
            ..Default::default()
        },
    );
    // SAFETY: The caller supplies writable aligned output; no references survive.
    unsafe {
        output.write(rect);
    }
    true
}

/// Change a settled selection's aspect, preserving the shipping center/min rules.
///
/// # Safety
/// Same output-storage contract as captures_selection_drag_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_selection_constrain_v1(
    rect: Rect,
    bounds: Bounds,
    aspect: f64,
    output: *mut Rect,
) -> bool {
    if output.is_null() || !valid(rect, bounds, aspect) {
        return false;
    }
    let result = selection::constrain(rect, (aspect > 0.).then_some(aspect), Some(bounds), 16.);
    // SAFETY: The caller supplies writable aligned output; no references survive.
    unsafe {
        output.write(result);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_validates_input_and_leaves_output_unchanged_on_failure() {
        let p = Point { x: 3., y: 5. };
        let initial = Rect {
            x: 3.,
            y: 5.,
            width: 40.,
            height: 20.,
        };
        let bounds = Bounds {
            width: 101.,
            height: 83.,
        };
        let mut output = initial;
        for (mode, aspect, point) in [
            (6, 0., p),
            (0, -1., p),
            (0, f64::NAN, p),
            (
                0,
                0.,
                Point {
                    x: f64::INFINITY,
                    ..p
                },
            ),
        ] {
            // SAFETY: output is an initialized, aligned local Rect.
            assert!(!unsafe {
                captures_selection_drag_v1(
                    mode,
                    p,
                    point,
                    initial,
                    bounds,
                    aspect,
                    false,
                    &mut output,
                )
            });
            assert_eq!(output, initial);
        }
        // SAFETY: null is explicitly rejected before dereferencing.
        assert!(!unsafe {
            captures_selection_constrain_v1(initial, bounds, 1., std::ptr::null_mut())
        });
        // SAFETY: output remains valid local storage.
        assert!(unsafe { captures_selection_constrain_v1(initial, bounds, 1., &mut output) });
        assert_eq!(
            output,
            Rect {
                x: 13.,
                y: 5.,
                width: 20.,
                height: 20.
            }
        );
    }
}
