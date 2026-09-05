mod conceal_policy;
mod cursor_policy;

pub use conceal_policy::should_conceal_documents_for_capture_activation;
pub use cursor_policy::{
    CaptureCursor, CaptureCursorEvent, CaptureCursorKind, CaptureCursorMonitorAction,
    ThumbnailHoverCursor, capture_cursor_monitor_action, capture_surface_focus_retry_allowed,
    cursor_claim_panel_should_resign_key, cursor_claim_panel_should_show,
    overlay_prepare_keeps_native_cursor, region_cursor_claim_waits_for_freeze_frame,
    region_shortcut_claims_cursor_on_press, suppress_document_cursor_rects_for_thumbnail,
    thumbnail_may_take_key_window, thumbnail_passthrough_disables_cursor_rects,
    thumbnail_poll_is_live, thumbnail_resets_cursor_on_exit, thumbnail_unpolled_hover,
};

#[cfg(target_os = "macos")]
mod macos_window;

#[cfg(target_os = "macos")]
pub use macos_window::*;
