mod cursor_policy;

pub use cursor_policy::{CaptureCursor, CaptureCursorKind};

#[cfg(target_os = "macos")]
mod macos_window;

#[cfg(target_os = "macos")]
pub use macos_window::*;
