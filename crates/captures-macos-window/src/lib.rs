mod conceal_policy;
mod cursor_policy;

pub use conceal_policy::should_conceal_documents_for_capture_activation;
pub use cursor_policy::{
    CaptureCursor, CaptureCursorKind, ThumbnailHoverCursor, overlay_prepare_keeps_native_cursor,
    region_shortcut_claims_cursor_on_press, suppress_document_cursor_rects_for_thumbnail,
    thumbnail_may_take_key_window, thumbnail_unpolled_hover_when_inactive,
};

#[cfg(target_os = "macos")]
mod macos_window;

#[cfg(target_os = "macos")]
pub use macos_window::*;
