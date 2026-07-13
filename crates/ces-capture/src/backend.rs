use image::RgbaImage;
use xcap::{Monitor, Window};

use crate::{
    error::{CaptureError, CaptureResult},
    model::{DisplayDescriptor, DisplayFrame, WindowDescriptor},
};

#[derive(Default)]
pub struct XcapBackend;

impl XcapBackend {
    pub fn ensure_permission(&self) -> CaptureResult<()> {
        #[cfg(target_os = "macos")]
        {
            let access = core_graphics::access::ScreenCaptureAccess;
            if !access.preflight() && !access.request() {
                return Err(CaptureError::PermissionDenied);
            }
        }

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

        Window::all()
            .map_err(|error| CaptureError::Backend(error.to_string()))?
            .into_iter()
            .filter_map(|window| {
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

                let display_id = Monitor::from_point(x, y).ok()?.id().ok()?;

                Some(Ok(WindowDescriptor {
                    id: window.id().ok()?.to_string(),
                    title: window.title().ok()?,
                    app_name: window.app_name().ok(),
                    x,
                    y,
                    width,
                    height,
                    display_id: display_id.to_string(),
                }))
            })
            .collect()
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
    std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_none()
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
