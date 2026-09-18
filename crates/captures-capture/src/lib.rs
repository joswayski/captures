#![forbid(unsafe_code)]

mod backend;
mod cursor;
mod error;
mod geometry;
#[cfg(target_os = "macos")]
mod macos;
mod model;
mod window;

pub use backend::XcapBackend;
pub use cursor::{
    CursorImage, PointerCursor, overlay_pointer_cursor, overlay_pointer_cursor_in_crop,
    overlay_pointer_cursor_on_window, pointer_cursor, pointer_position, screenshot_pointer_scale,
};
pub use error::{CaptureError, CaptureResult};
pub use geometry::{LogicalRect, PhysicalRect};
pub use model::{CaptureMode, DisplayDescriptor, DisplayFrame, WindowDescriptor};
pub use window::{image_is_effectively_blank, resolve_window_capture, window_display_crop_is_safe};
