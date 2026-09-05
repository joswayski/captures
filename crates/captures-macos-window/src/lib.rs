mod conceal_policy;
mod cursor_policy;

pub use conceal_policy::should_conceal_documents_for_capture_activation;
pub use cursor_policy::{
    CaptureCursor, CaptureCursorEvent, CaptureCursorKind, CaptureCursorMonitorAction,
    ThumbnailHoverCursor, capture_cursor_monitor_action, capture_escape_should_dispatch,
    capture_surface_focus_retry_allowed, cursor_claim_panel_should_resign_key,
    cursor_claim_panel_should_show, macos_key_code_is_escape, overlay_prepare_keeps_native_cursor,
    region_shortcut_claims_cursor_on_press, suppress_document_cursor_rects_for_thumbnail,
    thumbnail_foreign_mouse_click_must_resign_key, thumbnail_may_take_key_window,
    thumbnail_passthrough_disables_cursor_rects, thumbnail_passthrough_must_resign_key,
    thumbnail_poll_is_live, thumbnail_refresh_must_not_force_hit_testing,
    thumbnail_resets_cursor_on_exit, thumbnail_resign_active_may_retake_key,
    thumbnail_stale_poll_may_disable_click_through, thumbnail_stale_poll_may_take_key_window,
    thumbnail_unpolled_hover,
};

#[cfg(target_os = "macos")]
mod macos_window;

#[cfg(target_os = "macos")]
pub use macos_window::*;
