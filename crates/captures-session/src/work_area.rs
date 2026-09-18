//! Audited native monitor query; UI crates need not accept Win32 unsafe code.

/// Return the usable area `(x, y, width, height)` of the monitor whose full
/// physical desktop bounds exactly match `full`. Call from the same DPI-aware
/// context used to enumerate the monitor (native hosts use per-monitor awareness).
/// No nearest-monitor fallback, guessed taskbar margin, ownership transfer or
/// DPI conversion occurs. Invalid/stale bounds and OS query failures return None.
#[cfg(target_os = "windows")]
pub fn windows_monitor_work_area(full: (i32, i32, u32, u32)) -> Option<(i32, i32, u32, u32)> {
    use windows_sys::Win32::{
        Foundation::RECT,
        Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromRect},
    };

    let expected = rect_edges(full)?;
    let rect = RECT {
        left: expected[0],
        top: expected[1],
        right: expected[2],
        bottom: expected[3],
    };
    // SAFETY: rect is initialized readable stack storage; the API borrows it
    // for this call. The returned monitor is an OS-owned handle, not allocated.
    let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONULL) };
    if monitor.is_null() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: monitor came from MonitorFromRect; info is writable, correctly
    // sized stack storage. Removal/reconfiguration yields failure, not ownership.
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    matching_work_area(
        expected,
        [
            info.rcMonitor.left,
            info.rcMonitor.top,
            info.rcMonitor.right,
            info.rcMonitor.bottom,
        ],
        [
            info.rcWork.left,
            info.rcWork.top,
            info.rcWork.right,
            info.rcWork.bottom,
        ],
    )
}

fn rect_edges((x, y, width, height): (i32, i32, u32, u32)) -> Option<[i32; 4]> {
    if width == 0 || height == 0 {
        return None;
    }
    Some([
        x,
        y,
        i32::try_from(i64::from(x) + i64::from(width)).ok()?,
        i32::try_from(i64::from(y) + i64::from(height)).ok()?,
    ])
}

fn matching_work_area(
    expected: [i32; 4],
    monitor: [i32; 4],
    work: [i32; 4],
) -> Option<(i32, i32, u32, u32)> {
    // MonitorFromRect selects the largest intersection. Do not accept a different
    // display after hotplug or a logical/physical coordinate mismatch.
    if monitor != expected
        || work[0] < monitor[0]
        || work[1] < monitor[1]
        || work[2] > monitor[2]
        || work[3] > monitor[3]
        || work[2] <= work[0]
        || work[3] <= work[1]
    {
        return None;
    }
    Some((
        work[0],
        work[1],
        u32::try_from(i64::from(work[2]) - i64::from(work[0])).ok()?,
        u32::try_from(i64::from(work[3]) - i64::from(work[1])).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_negative_origin_and_asymmetric_taskbar_are_preserved() {
        let full = rect_edges((-2560, -240, 2560, 1440)).unwrap();
        assert_eq!(full, [-2560, -240, 0, 1200]);
        assert_eq!(
            matching_work_area(full, full, [-2488, -210, 0, 1158]),
            Some((-2488, -210, 2488, 1368))
        );
        assert_eq!(
            matching_work_area(full, full, full),
            Some((-2560, -240, 2560, 1440)),
            "auto-hidden taskbar reserve belongs to shared preview policy, not the OS query"
        );
    }

    #[test]
    fn rejects_empty_overflowing_and_reconfigured_bounds() {
        assert_eq!(rect_edges((0, 0, 0, 720)), None);
        assert_eq!(rect_edges((0, 0, 1280, 0)), None);
        assert_eq!(rect_edges((i32::MAX - 1, 0, 2, 1)), None);
        assert_eq!(rect_edges((0, i32::MAX - 1, 1, 2)), None);
        assert_eq!(
            rect_edges((i32::MIN, 0, u32::MAX, 1)),
            Some([i32::MIN, 0, i32::MAX, 1])
        );
        let full = [0, 0, 1920, 1080];
        assert_eq!(
            matching_work_area(full, [1920, 0, 3840, 1080], [1920, 0, 3840, 1040]),
            None
        );
        for invalid in [
            [-1, 0, 1920, 1080],
            [0, -1, 1920, 1080],
            [0, 0, 1921, 1080],
            [0, 0, 1920, 1081],
            [100, 0, 100, 1080],
            [0, 1080, 1920, 10],
        ] {
            assert_eq!(matching_work_area(full, full, invalid), None);
        }
    }
}
