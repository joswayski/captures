#![forbid(unsafe_code)]

mod backend;
mod cursor;
mod error;
mod geometry;
mod model;

pub use backend::XcapBackend;
pub use cursor::{
    overlay_pointer_cursor, overlay_pointer_cursor_in_crop, overlay_pointer_cursor_on_window,
    screenshot_pointer_scale,
};
pub use error::{CaptureError, CaptureResult};
pub use geometry::{LogicalRect, PhysicalRect};
pub use model::{CaptureMode, DisplayDescriptor, WindowDescriptor};
