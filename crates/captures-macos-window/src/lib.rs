mod conceal_policy;
mod cursor_policy;

pub use conceal_policy::should_conceal_documents_for_capture_activation;
pub use cursor_policy::{
    CaptureCursor, CaptureCursorEvent, CaptureCursorKind, CaptureCursorMonitorAction,
    ThumbnailHoverCursor, capture_cursor_monitor_action, overlay_prepare_keeps_native_cursor,
    region_shortcut_claims_cursor_on_press, should_restore_thumbnail_css_cursor_rects,
    suppress_document_cursor_rects_for_thumbnail, thumbnail_css_fallback_restores_cursor_rects,
    thumbnail_may_take_key_window, thumbnail_passthrough_disables_cursor_rects,
    thumbnail_poll_is_live, thumbnail_resets_cursor_on_exit, thumbnail_unpolled_hover,
};

#[cfg(target_os = "macos")]
mod macos_window;

#[cfg(target_os = "macos")]
pub use macos_window::*;
