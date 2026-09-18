#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[cfg(target_os = "linux")]
pub fn for_monitor(full: PhysicalRect) -> Option<PhysicalRect> {
    use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};

    if std::env::var("XDG_SESSION_TYPE").is_ok_and(|session| session == "wayland") {
        return None;
    }
    let (connection, screen) = x11rb::connect(None).ok()?;
    let root = connection.setup().roots.get(screen)?.root;
    let current_desktop = connection
        .intern_atom(true, b"_NET_CURRENT_DESKTOP")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let work_area = connection
        .intern_atom(true, b"_NET_WORKAREA")
        .ok()?
        .reply()
        .ok()?
        .atom;
    if current_desktop == 0 || work_area == 0 {
        return None;
    }
    let desktop = connection
        .get_property(
            false,
            root,
            current_desktop,
            x11rb::protocol::xproto::AtomEnum::CARDINAL,
            0,
            1,
        )
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next()? as usize;
    let values = connection
        .get_property(
            false,
            root,
            work_area,
            x11rb::protocol::xproto::AtomEnum::CARDINAL,
            (desktop * 4) as u32,
            4,
        )
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .collect::<Vec<_>>();
    let [x, y, width, height] = values.as_slice() else {
        return None;
    };
    intersect(
        full,
        PhysicalRect {
            x: *x as i32,
            y: *y as i32,
            width: *width,
            height: *height,
        },
    )
}

#[cfg(target_os = "windows")]
pub fn for_monitor(full: PhysicalRect) -> Option<PhysicalRect> {
    captures_session::windows_monitor_work_area((full.x, full.y, full.width, full.height)).map(
        |(x, y, width, height)| PhysicalRect {
            x,
            y,
            width,
            height,
        },
    )
}

#[cfg(any(target_os = "linux", test))]
fn intersect(left: PhysicalRect, right: PhysicalRect) -> Option<PhysicalRect> {
    let x = left.x.max(right.x);
    let y = left.y.max(right.y);
    let right_edge = (i64::from(left.x) + i64::from(left.width))
        .min(i64::from(right.x) + i64::from(right.width));
    let bottom_edge = (i64::from(left.y) + i64::from(left.height))
        .min(i64::from(right.y) + i64::from(right.height));
    let width = u32::try_from(right_edge - i64::from(x)).ok()?;
    let height = u32::try_from(bottom_edge - i64::from(y)).ok()?;
    (width > 0 && height > 0).then_some(PhysicalRect {
        x,
        y,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_desktop_work_area_to_the_target_monitor() {
        let monitor = PhysicalRect {
            x: -1_920,
            y: 0,
            width: 1_920,
            height: 1_080,
        };
        let desktop_work = PhysicalRect {
            x: -1_920,
            y: 24,
            width: 3_840,
            height: 1_016,
        };
        assert_eq!(
            intersect(monitor, desktop_work),
            Some(PhysicalRect {
                x: -1_920,
                y: 24,
                width: 1_920,
                height: 1_016,
            })
        );
    }
}
