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
#[cfg(target_os = "macos")]
pub use window::macos_window_is_capture_overlay;
pub use window::{
    RECORDING_REGION_INDICATOR_TITLE, WindowPickRole, WindowSelectionTargets, capture_buffer_scale,
    classify_windows_for_display, image_is_effectively_blank, mask_macos_window_corners,
    refine_window_chrome_from_snapshot, resolve_window_capture, window_display_crop_is_safe,
    window_is_capturable, window_physical_rect, window_pick_role,
    windows_window_is_capture_overlay,
};
