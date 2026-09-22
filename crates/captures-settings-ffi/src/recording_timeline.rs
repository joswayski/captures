//! Allocation-free recording timeline geometry for native hosts.

use captures_app::recording_timeline::{
    TimelineTrimDrag, TimelineTrimEdge, TimelineTrimUpdate, timeline_ratio,
    timeline_time_at_client_x,
};

/// # Safety
/// Output is null or writable aligned storage for one double. False leaves it untouched.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_timeline_ratio_v1(
    time_ms: f64,
    duration_ms: f64,
    output: *mut f64,
) -> bool {
    if output.is_null() {
        return false;
    }
    let Some(ratio) = timeline_ratio(time_ms, duration_ms) else {
        return false;
    };
    // SAFETY: caller supplies writable output; all inputs and results are copied.
    unsafe { output.write(ratio) };
    true
}

/// # Safety
/// Output is null or writable aligned storage for one double. False leaves it untouched.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_timeline_time_at_x_v1(
    client_x: f64,
    track_left: f64,
    track_width: f64,
    duration_ms: f64,
    output: *mut f64,
) -> bool {
    if output.is_null() {
        return false;
    }
    let Some(time_ms) = timeline_time_at_client_x(client_x, track_left, track_width, duration_ms)
    else {
        return false;
    };
    // SAFETY: caller supplies writable output; all inputs and results are copied.
    unsafe { output.write(time_ms) };
    true
}

/// Start one trim-handle gesture. Edge 0 is start and edge 1 is end.
///
/// # Safety
/// Output is null or writable aligned drag storage. False leaves it untouched.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_timeline_trim_begin_v1(
    edge: u8,
    pointer_x: f64,
    trim_start_ms: f64,
    trim_end_ms: f64,
    duration_ms: f64,
    output: *mut TimelineTrimDrag,
) -> bool {
    if output.is_null() {
        return false;
    }
    let Ok(edge) = TimelineTrimEdge::try_from(edge) else {
        return false;
    };
    let Some(drag) =
        TimelineTrimDrag::begin(edge, pointer_x, trim_start_ms, trim_end_ms, duration_ms)
    else {
        return false;
    };
    // SAFETY: caller supplies writable output; drag has no borrowed state.
    unsafe { output.write(drag) };
    true
}

/// Apply one pointer sample and return the next drag state plus staged time.
///
/// # Safety
/// Output is null or writable aligned update storage. False leaves it untouched.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_timeline_trim_update_v1(
    drag: TimelineTrimDrag,
    client_x: f64,
    track_left: f64,
    track_width: f64,
    output: *mut TimelineTrimUpdate,
) -> bool {
    if output.is_null() {
        return false;
    }
    let Some(update) = drag.update(client_x, track_left, track_width) else {
        return false;
    };
    // SAFETY: caller supplies writable output; update has no borrowed state.
    unsafe { output.write(update) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    fn sentinel_drag() -> TimelineTrimDrag {
        TimelineTrimDrag {
            start_time_ms: 11.,
            start_x: 12.,
            last_x: 13.,
            min_time_ms: 14.,
            max_time_ms: 15.,
            duration_ms: 16.,
            dragging: true,
        }
    }

    #[test]
    fn timeline_abi_copies_threshold_glitch_and_recovery_state() {
        let mut ratio = -1.;
        let mut time = -1.;
        let mut drag = sentinel_drag();
        // SAFETY: each output is live writable storage for the call.
        unsafe {
            assert!(captures_recording_timeline_ratio_v1(
                4_375., 8_750., &mut ratio
            ));
            assert_eq!(ratio, 0.5);
            assert!(captures_recording_timeline_time_at_x_v1(
                500., 0., 1_000., 8_750., &mut time
            ));
            assert_eq!(time, 4_375.);
            assert!(captures_recording_timeline_trim_begin_v1(
                0,
                228.571_428_571_428_58,
                2_000.,
                6_750.,
                8_750.,
                &mut drag,
            ));
        }
        let mut update = TimelineTrimUpdate {
            drag: sentinel_drag(),
            time_ms: -1.,
        };
        // SAFETY: update remains writable for each call and copied drag values stay live.
        unsafe {
            assert!(captures_recording_timeline_trim_update_v1(
                drag,
                drag.start_x + 2.999,
                0.,
                1_000.,
                &mut update,
            ));
            assert!(!update.drag.dragging);
            assert_eq!(update.time_ms, 2_000.);
            assert!(captures_recording_timeline_trim_update_v1(
                update.drag,
                9_000.,
                0.,
                1_000.,
                &mut update,
            ));
            assert!(update.drag.dragging);
            assert_eq!(update.time_ms, 2_000.);
            assert_eq!(update.drag.last_x, drag.last_x);
            assert!(captures_recording_timeline_trim_update_v1(
                update.drag,
                drag.start_x + 50.,
                0.,
                1_000.,
                &mut update,
            ));
        }
        assert!((update.time_ms - 2_437.5).abs() < 1e-10);
        assert_eq!(update.drag.last_x, drag.start_x + 50.);
    }

    #[test]
    fn invalid_timeline_abi_inputs_leave_outputs_untouched() {
        let sentinel = sentinel_drag();
        let mut drag = sentinel;
        let mut scalar = 17.;
        let mut update = TimelineTrimUpdate {
            drag: sentinel,
            time_ms: 18.,
        };
        // SAFETY: non-null outputs are writable and null outputs are supported failures.
        unsafe {
            assert!(!captures_recording_timeline_ratio_v1(
                f64::NAN,
                1.,
                &mut scalar,
            ));
            assert!(!captures_recording_timeline_time_at_x_v1(
                0.,
                0.,
                f64::INFINITY,
                1.,
                &mut scalar,
            ));
            assert!(!captures_recording_timeline_trim_begin_v1(
                2, 0., 0., 1., 1., &mut drag,
            ));
            assert!(!captures_recording_timeline_trim_update_v1(
                sentinel,
                f64::NAN,
                0.,
                10.,
                &mut update,
            ));
            assert!(!captures_recording_timeline_ratio_v1(
                0.,
                1.,
                ptr::null_mut()
            ));
            assert!(!captures_recording_timeline_trim_begin_v1(
                0,
                0.,
                0.,
                1.,
                1.,
                ptr::null_mut(),
            ));
        }
        assert_eq!(scalar, 17.);
        assert_eq!(drag, sentinel);
        assert_eq!(update.drag, sentinel);
        assert_eq!(update.time_ms, 18.);
    }
}
