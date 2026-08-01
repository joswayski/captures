use image::RgbaImage;
use xcap::{Monitor, Window};

use crate::{
    error::{CaptureError, CaptureResult},
    model::{DisplayDescriptor, DisplayFrame, WindowDescriptor},
};

#[derive(Default)]
pub struct XcapBackend;

impl XcapBackend {
    pub fn ensure_permission(&self, request_access: bool) -> CaptureResult<()> {
        #[cfg(target_os = "macos")]
        {
            let access = core_graphics::access::ScreenCaptureAccess;
            if !access.preflight() {
                if request_access {
                    if access.request() {
                        return Ok(());
                    }
                    return Err(CaptureError::PermissionRequestStarted);
                }
                return Err(CaptureError::PermissionDenied);
            }
        }

        #[cfg(not(target_os = "macos"))]
        let _ = request_access;

        Ok(())
    }

    pub fn displays(&self) -> CaptureResult<Vec<DisplayDescriptor>> {
        Monitor::all()
            .map_err(|error| CaptureError::Backend(error.to_string()))?
            .into_iter()
            .map(|monitor| {
                Ok(DisplayDescriptor {
                    id: monitor
                        .id()
                        .map_err(|error| CaptureError::Backend(error.to_string()))?
                        .to_string(),
                    name: monitor
                        .friendly_name()
                        .map_err(|error| CaptureError::Backend(error.to_string()))?,
                    x: monitor
                        .x()
                        .map_err(|error| CaptureError::Backend(error.to_string()))?,
                    y: monitor
                        .y()
                        .map_err(|error| CaptureError::Backend(error.to_string()))?,
                    width: monitor
                        .width()
                        .map_err(|error| CaptureError::Backend(error.to_string()))?,
                    height: monitor
                        .height()
                        .map_err(|error| CaptureError::Backend(error.to_string()))?,
                    scale_factor: f64::from(
                        monitor
                            .scale_factor()
                            .map_err(|error| CaptureError::Backend(error.to_string()))?,
                    ),
                    is_primary: monitor
                        .is_primary()
                        .map_err(|error| CaptureError::Backend(error.to_string()))?,
                })
            })
            .collect()
    }

    pub fn capture_display(&self, id: &str) -> CaptureResult<DisplayFrame> {
        let monitor = Self::find_monitor(id)?;
        let descriptor = descriptor_for_monitor(&monitor)?;
        let image = monitor
            .capture_image()
            .map_err(|error| CaptureError::Backend(error.to_string()))?;
        Ok(DisplayFrame { descriptor, image })
    }

    pub fn windows(&self) -> CaptureResult<Vec<WindowDescriptor>> {
        #[cfg(target_os = "linux")]
        if wayland_without_x11() {
            return Err(CaptureError::Unsupported);
        }

        #[cfg(target_os = "linux")]
        let monitors = Monitor::all().map_err(|error| CaptureError::Backend(error.to_string()))?;

        let native_windows =
            Window::all().map_err(|error| CaptureError::Backend(error.to_string()))?;
        let fallback_front = i32::try_from(native_windows.len()).unwrap_or(i32::MAX);
        let mut windows = native_windows
            .into_iter()
            .enumerate()
            .filter_map(|(index, window)| {
                let is_minimized = window.is_minimized().ok()?;
                if is_minimized {
                    return None;
                }

                let x = window.x().ok()?;
                let y = window.y().ok()?;
                let width = window.width().ok()?;
                let height = window.height().ok()?;
                if width == 0 || height == 0 {
                    return None;
                }

                #[cfg(target_os = "linux")]
                let (x, y, width, height, display_id) =
                    linux_window_geometry(&monitors, x, y, width, height)?;
                #[cfg(not(target_os = "linux"))]
                let display_id = Monitor::from_point(x, y).ok()?.id().ok()?;

                let fallback_z =
                    fallback_front.saturating_sub(i32::try_from(index).unwrap_or(i32::MAX));
                Some(Ok(WindowDescriptor {
                    id: window.id().ok()?.to_string(),
                    title: window.title().ok()?,
                    app_name: window.app_name().ok(),
                    z_order: window.z().unwrap_or(fallback_z),
                    x,
                    y,
                    width,
                    height,
                    display_id: display_id.to_string(),
                    corner_radius: None,
                }))
            })
            .collect::<CaptureResult<Vec<_>>>()?;
        windows.sort_by_key(|window| std::cmp::Reverse(window.z_order));
        Ok(windows)
    }

    pub fn capture_window(&self, id: &str) -> CaptureResult<RgbaImage> {
        #[cfg(target_os = "linux")]
        if wayland_without_x11() {
            return Err(CaptureError::Unsupported);
        }

        let window = Window::all()
            .map_err(|error| CaptureError::Backend(error.to_string()))?
            .into_iter()
            .find(|candidate| {
                candidate
                    .id()
                    .map(|value| value.to_string() == id)
                    .unwrap_or(false)
            })
            .ok_or(CaptureError::TargetUnavailable)?;

        window
            .capture_image()
            .map_err(|error| CaptureError::Backend(error.to_string()))
    }

    fn find_monitor(id: &str) -> CaptureResult<Monitor> {
        Monitor::all()
            .map_err(|error| CaptureError::Backend(error.to_string()))?
            .into_iter()
            .find(|monitor| {
                monitor
                    .id()
                    .map(|value| value.to_string() == id)
                    .unwrap_or(false)
            })
            .ok_or(CaptureError::TargetUnavailable)
    }
}

#[cfg(target_os = "linux")]
fn wayland_without_x11() -> bool {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("XDG_SESSION_TYPE")
            .is_some_and(|session| session.to_string_lossy().eq_ignore_ascii_case("wayland"));
    wayland && std::env::var_os("DISPLAY").is_none()
}

#[cfg(target_os = "linux")]
fn linux_window_geometry(
    monitors: &[Monitor],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Option<(i32, i32, u32, u32, String)> {
    let monitor = monitors.iter().max_by(|left, right| {
        linux_window_monitor_overlap(left, x, y, width, height)
            .partial_cmp(&linux_window_monitor_overlap(right, x, y, width, height))
            .unwrap_or(std::cmp::Ordering::Equal)
    })?;
    if linux_window_monitor_overlap(monitor, x, y, width, height) <= 0.0 {
        return None;
    }
    let scale = f64::from(monitor.scale_factor().ok()?).max(1.0);
    let display_id = monitor.id().ok()?.to_string();
    let (x, y, width, height) = logical_window_rect(x, y, width, height, scale);
    Some((x, y, width, height, display_id))
}

#[cfg(target_os = "linux")]
fn linux_window_monitor_overlap(monitor: &Monitor, x: i32, y: i32, width: u32, height: u32) -> f64 {
    let Ok(scale_factor) = monitor.scale_factor() else {
        return 0.0;
    };
    let scale = f64::from(scale_factor).max(1.0);
    let (Ok(monitor_x), Ok(monitor_y), Ok(monitor_width), Ok(monitor_height)) =
        (monitor.x(), monitor.y(), monitor.width(), monitor.height())
    else {
        return 0.0;
    };
    let left = f64::from(x).max(f64::from(monitor_x) * scale);
    let top = f64::from(y).max(f64::from(monitor_y) * scale);
    let right = (f64::from(x) + f64::from(width))
        .min((f64::from(monitor_x) + f64::from(monitor_width)) * scale);
    let bottom = (f64::from(y) + f64::from(height))
        .min((f64::from(monitor_y) + f64::from(monitor_height)) * scale);
    (right - left).max(0.0) * (bottom - top).max(0.0)
}

#[cfg(any(target_os = "linux", test))]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
fn logical_window_rect(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale_factor: f64,
) -> (i32, i32, u32, u32) {
    let scale = scale_factor.max(1.0);
    (
        (f64::from(x) / scale).round() as i32,
        (f64::from(y) / scale).round() as i32,
        (f64::from(width) / scale).round().max(1.0) as u32,
        (f64::from(height) / scale).round().max(1.0) as u32,
    )
}

fn descriptor_for_monitor(monitor: &Monitor) -> CaptureResult<DisplayDescriptor> {
    Ok(DisplayDescriptor {
        id: monitor
            .id()
            .map_err(|error| CaptureError::Backend(error.to_string()))?
            .to_string(),
        name: monitor
            .friendly_name()
            .map_err(|error| CaptureError::Backend(error.to_string()))?,
        x: monitor
            .x()
            .map_err(|error| CaptureError::Backend(error.to_string()))?,
        y: monitor
            .y()
            .map_err(|error| CaptureError::Backend(error.to_string()))?,
        width: monitor
            .width()
            .map_err(|error| CaptureError::Backend(error.to_string()))?,
        height: monitor
            .height()
            .map_err(|error| CaptureError::Backend(error.to_string()))?,
        scale_factor: f64::from(
            monitor
                .scale_factor()
                .map_err(|error| CaptureError::Backend(error.to_string()))?,
        ),
        is_primary: monitor
            .is_primary()
            .map_err(|error| CaptureError::Backend(error.to_string()))?,
    })
}

#[cfg(test)]
mod tests {
    use super::logical_window_rect;

    #[test]
    fn linux_hidpi_window_geometry_uses_logical_coordinates() {
        assert_eq!(
            logical_window_rect(4_320, 240, 1_600, 1_000, 2.0),
            (2_160, 120, 800, 500)
        );
        assert_eq!(
            logical_window_rect(-1_920, -120, 1_200, 900, 1.5),
            (-1_280, -80, 800, 600)
        );
    }
}
