/// Cursor a capture overlay or capture menu should show for the current mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CaptureCursorKind {
    /// Display selection keeps the default arrow.
    Arrow = 0,
    /// Region selection.
    Crosshair = 1,
    /// Window selection uses the CSS camera cursor through WebKit.
    WebView = 2,
}

/// How a capture surface claims the pointer when it appears.
///
/// `native_owned` is true when AppKit should keep WebKit cursor rectangles off
/// (the full-screen region overlay has no HUD chrome that needs CSS cursors).
/// The capture menu leaves rectangles on so grab/pointer still apply on the panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureCursor {
    pub kind: CaptureCursorKind,
    pub native_owned: bool,
}

impl CaptureCursor {
    #[must_use]
    pub const fn overlay(is_region: bool) -> Self {
        if is_region {
            Self::overlay_region()
        } else {
            Self::overlay_window()
        }
    }

    #[must_use]
    pub const fn overlay_region() -> Self {
        Self {
            kind: CaptureCursorKind::Crosshair,
            native_owned: true,
        }
    }

    #[must_use]
    pub const fn overlay_window() -> Self {
        Self {
            kind: CaptureCursorKind::WebView,
            native_owned: false,
        }
    }

    #[must_use]
    pub const fn selector(is_region: bool, is_window: bool) -> Self {
        if is_region {
            Self::selector_region()
        } else if is_window {
            Self::selector_window()
        } else {
            Self::selector_display()
        }
    }

    #[must_use]
    pub const fn selector_region() -> Self {
        Self {
            kind: CaptureCursorKind::Crosshair,
            native_owned: false,
        }
    }

    #[must_use]
    pub const fn selector_window() -> Self {
        Self {
            kind: CaptureCursorKind::WebView,
            native_owned: false,
        }
    }

    #[must_use]
    pub const fn selector_display() -> Self {
        Self {
            kind: CaptureCursorKind::Arrow,
            native_owned: false,
        }
    }

    #[must_use]
    pub const fn disables_cursor_rects(self) -> bool {
        self.native_owned
    }

    /// Cursor kind for the native mouse-move tracker. When WebKit owns cursor
    /// rectangles, its CSS must be the only per-move writer or menu cursors
    /// flicker between the target cursor and grab/pointer controls.
    #[must_use]
    pub const fn tracked_kind(self) -> CaptureCursorKind {
        if self.native_owned {
            self.kind
        } else {
            CaptureCursorKind::WebView
        }
    }

    /// Shortcut-modifier transitions restore the arrow. Native-owned overlays
    /// re-apply `NSCursor`; selector/window modes refresh WebKit rectangles so
    /// panel grab/pointer and CSS camera cursors are not overwritten.
    #[must_use]
    pub const fn reasserts_native_cursor_on_modifiers(self) -> bool {
        self.native_owned
    }

    /// Mouse-move monitors keep a native-owned region crosshair from snapping
    /// back to the arrow. CSS-owned surfaces must not rewrite `NSCursor` or
    /// rebuild WebKit rectangles on every move — that races panel grab/pointer
    /// with the default arrow.
    #[must_use]
    pub const fn reasserts_native_cursor_on_mouse_move(self) -> bool {
        self.native_owned
    }
}

/// Capture-cursor event kinds mirrored by the AppKit local/global monitors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureCursorEvent {
    FlagsChanged,
    MouseMoved,
}

/// Work a capture-cursor monitor should do for one AppKit event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureCursorMonitorAction {
    Ignore,
    ReassertNative,
    RefreshWebKitRects,
}

/// Selector region uses `CaptureCursorKind::Crosshair` with `native_owned:
/// false` so the dimmed surface can show a crosshair in CSS without locking
/// out panel grab/pointer. Callers must not treat Crosshair as "always
/// reassert native."
#[must_use]
pub const fn capture_cursor_monitor_action(
    event: CaptureCursorEvent,
    cursor: CaptureCursor,
) -> CaptureCursorMonitorAction {
    match event {
        CaptureCursorEvent::FlagsChanged => {
            if cursor.reasserts_native_cursor_on_modifiers() {
                CaptureCursorMonitorAction::ReassertNative
            } else {
                CaptureCursorMonitorAction::RefreshWebKitRects
            }
        }
        CaptureCursorEvent::MouseMoved => {
            if cursor.reasserts_native_cursor_on_mouse_move() {
                CaptureCursorMonitorAction::ReassertNative
            } else {
                CaptureCursorMonitorAction::Ignore
            }
        }
    }
}

/// Mini-preview cursor kinds mirrored by the native AppKit tracker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThumbnailHoverCursor {
    Default,
    Pointer,
    Grab,
}

impl ThumbnailHoverCursor {
    #[must_use]
    pub const fn is_interactive(self) -> bool {
        !matches!(self, Self::Default)
    }

    /// Pointer is already over the stack, but JS may not have hit-tested yet
    /// (inactive WKWebView timers are frozen). Show a pointing hand immediately
    /// instead of leaving the frontmost app's arrow until the panel is focused.
    #[must_use]
    pub const fn unpolled_hover(self) -> Self {
        match self {
            Self::Default => Self::Pointer,
            other => other,
        }
    }

    /// Grab/pointer are AppKit-owned. Default is a hole in the always-on-top
    /// panel (collapsed stack, padding, exiting cards) and must not install an
    /// arrow or WebKit cursor rectangles, or apps underneath lose hover cursors
    /// even after clicks already pass through.
    #[must_use]
    pub const fn claims_ns_cursor(self) -> bool {
        self.is_interactive()
    }
}

/// Whether the JavaScript thumbnail pointer poll is recent enough to trust.
#[must_use]
pub const fn thumbnail_poll_is_live(elapsed_ms: u64) -> bool {
    elapsed_ms <= 250
}

/// Use the hit-tested kind while the JavaScript pointer poll is live. When
/// WebKit timers are frozen, promote a pointer inside the stack immediately.
#[must_use]
pub const fn thumbnail_unpolled_hover(
    poll_is_live: bool,
    kind: ThumbnailHoverCursor,
) -> ThumbnailHoverCursor {
    if poll_is_live {
        kind
    } else {
        kind.unpolled_hover()
    }
}

/// Priming a reused overlay must not re-enable WebKit cursor rectangles after a
/// native-owned region crosshair has already been claimed.
#[must_use]
pub const fn overlay_prepare_keeps_native_cursor(native_owned: bool) -> bool {
    native_owned
}

/// Region screenshot (⌘⇧4) claims the crosshair on key-down, not after
/// modifiers come up or the freeze-frame paints.
#[must_use]
pub const fn region_shortcut_claims_cursor_on_press() -> bool {
    true
}

/// After a thumbnail click, key-on-hover stays latched off so an opening
/// editor can keep keyboard focus. Hovering from another app must still be
/// allowed to take key status without waiting for a click.
#[must_use]
pub const fn thumbnail_may_take_key_window(allowed_after_click: bool, app_is_active: bool) -> bool {
    allowed_after_click || !app_is_active
}

/// Click-through empty space still sits inside the panel frame after collapse.
/// `mouseExited` fires when `ignoresMouseEvents` flips, and stamping
/// `NSCursor::arrow` there leaves the arrow over whatever now receives clicks.
#[must_use]
pub const fn thumbnail_resets_cursor_on_exit() -> bool {
    false
}

/// Passthrough hover keeps WebKit cursor rectangles off so `cursor: default`
/// on the transparent panel cannot win over the app underneath.
#[must_use]
pub const fn thumbnail_passthrough_disables_cursor_rects() -> bool {
    true
}

/// Sleep/IPC recovery drops native tracking and uses CSS `:hover` until the
/// pointer poll is live again. Cursor rectangles disabled for a click-through
/// hole must be turned back on so grab/pointer CSS can apply during that
/// fallback.
#[must_use]
pub const fn thumbnail_css_fallback_restores_cursor_rects() -> bool {
    true
}

/// The concealed thumbnail panel stays ordered onscreen at zero alpha. Restoring
/// cursor rectangles there would steal hover cursors from the app underneath.
#[must_use]
pub const fn should_restore_thumbnail_css_cursor_rects(
    presented: bool,
    overlay_owns_cursor: bool,
) -> bool {
    thumbnail_css_fallback_restores_cursor_rects() && presented && !overlay_owns_cursor
}

/// A titled editor's cursor rectangles reset `NSCursor` to the arrow while the
/// pointer is still on a mini-preview control. Disable them for that window
/// until the pointer leaves the stack.
#[must_use]
pub const fn suppress_document_cursor_rects_for_thumbnail(
    thumbnail_cursor_interactive: bool,
    key_window_is_thumbnail: bool,
    key_window_is_titled_document: bool,
) -> bool {
    thumbnail_cursor_interactive && !key_window_is_thumbnail && key_window_is_titled_document
}

#[cfg(test)]
mod tests {
    use super::{
        CaptureCursor, CaptureCursorEvent, CaptureCursorKind, CaptureCursorMonitorAction,
        ThumbnailHoverCursor, capture_cursor_monitor_action, overlay_prepare_keeps_native_cursor,
        region_shortcut_claims_cursor_on_press, should_restore_thumbnail_css_cursor_rects,
        suppress_document_cursor_rects_for_thumbnail, thumbnail_css_fallback_restores_cursor_rects,
        thumbnail_may_take_key_window, thumbnail_passthrough_disables_cursor_rects,
        thumbnail_poll_is_live, thumbnail_resets_cursor_on_exit, thumbnail_unpolled_hover,
    };

    #[test]
    fn kind_discriminants_match_the_atomic_storage_map() {
        assert_eq!(CaptureCursorKind::Arrow as u8, 0);
        assert_eq!(CaptureCursorKind::Crosshair as u8, 1);
        assert_eq!(CaptureCursorKind::WebView as u8, 2);
    }

    #[test]
    fn overlay_region_owns_the_native_crosshair() {
        let cursor = CaptureCursor::overlay(true);
        assert_eq!(cursor.kind, CaptureCursorKind::Crosshair);
        assert!(cursor.native_owned);
        assert!(cursor.disables_cursor_rects());
    }

    #[test]
    fn overlay_window_leaves_css_cursor_rects_enabled() {
        let cursor = CaptureCursor::overlay(false);
        assert_eq!(cursor.kind, CaptureCursorKind::WebView);
        assert!(!cursor.native_owned);
        assert!(!cursor.disables_cursor_rects());
    }

    #[test]
    fn selector_region_shows_a_crosshair_without_locking_out_panel_css() {
        let cursor = CaptureCursor::selector(true, false);
        assert_eq!(cursor.kind, CaptureCursorKind::Crosshair);
        assert_eq!(cursor.tracked_kind(), CaptureCursorKind::WebView);
        assert!(!cursor.native_owned);
        assert!(!cursor.disables_cursor_rects());
    }

    #[test]
    fn selector_window_and_display_keep_css_cursor_rects() {
        let window = CaptureCursor::selector(false, true);
        assert_eq!(window.kind, CaptureCursorKind::WebView);
        assert_eq!(window.tracked_kind(), CaptureCursorKind::WebView);
        assert!(!window.native_owned);

        let display = CaptureCursor::selector(false, false);
        assert_eq!(display.kind, CaptureCursorKind::Arrow);
        assert_eq!(display.tracked_kind(), CaptureCursorKind::WebView);
        assert!(!display.native_owned);
    }

    #[test]
    fn modifier_changes_keep_css_cursors_on_the_capture_menu() {
        assert!(CaptureCursor::overlay_region().reasserts_native_cursor_on_modifiers());
        assert!(!CaptureCursor::overlay_window().reasserts_native_cursor_on_modifiers());
        assert!(!CaptureCursor::selector_region().reasserts_native_cursor_on_modifiers());
        assert!(!CaptureCursor::selector_window().reasserts_native_cursor_on_modifiers());
        assert!(!CaptureCursor::selector_display().reasserts_native_cursor_on_modifiers());
    }

    #[test]
    fn selector_crosshair_does_not_reassert_native_cursor_on_mouse_move() {
        let cursor = CaptureCursor::selector_region();
        assert_eq!(cursor.kind, CaptureCursorKind::Crosshair);
        assert!(!cursor.native_owned);
        assert!(!cursor.reasserts_native_cursor_on_mouse_move());
        assert_eq!(
            capture_cursor_monitor_action(CaptureCursorEvent::MouseMoved, cursor),
            CaptureCursorMonitorAction::Ignore,
        );
    }

    #[test]
    fn capture_menu_mouse_moves_leave_css_cursors_alone() {
        for cursor in [
            CaptureCursor::selector_region(),
            CaptureCursor::selector_window(),
            CaptureCursor::selector_display(),
            CaptureCursor::overlay_window(),
        ] {
            assert_eq!(
                capture_cursor_monitor_action(CaptureCursorEvent::MouseMoved, cursor),
                CaptureCursorMonitorAction::Ignore,
            );
            assert_eq!(
                capture_cursor_monitor_action(CaptureCursorEvent::FlagsChanged, cursor),
                CaptureCursorMonitorAction::RefreshWebKitRects,
            );
        }
    }

    #[test]
    fn region_overlay_reasserts_native_crosshair_on_mouse_move() {
        let cursor = CaptureCursor::overlay_region();
        assert!(cursor.reasserts_native_cursor_on_mouse_move());
        assert_eq!(
            capture_cursor_monitor_action(CaptureCursorEvent::MouseMoved, cursor),
            CaptureCursorMonitorAction::ReassertNative,
        );
        assert_eq!(
            capture_cursor_monitor_action(CaptureCursorEvent::FlagsChanged, cursor),
            CaptureCursorMonitorAction::ReassertNative,
        );
    }

    #[test]
    fn priming_a_region_overlay_keeps_the_native_crosshair() {
        assert!(overlay_prepare_keeps_native_cursor(
            CaptureCursor::overlay_region().native_owned
        ));
        assert!(!overlay_prepare_keeps_native_cursor(
            CaptureCursor::overlay_window().native_owned
        ));
        assert!(region_shortcut_claims_cursor_on_press());
    }

    #[test]
    fn thumbnail_poll_liveness_has_a_250ms_stale_threshold() {
        assert!(thumbnail_poll_is_live(0));
        assert!(thumbnail_poll_is_live(250));
        assert!(!thumbnail_poll_is_live(251));
    }

    #[test]
    fn unpolled_thumbnail_hover_uses_a_pointing_hand_when_poll_is_stale() {
        assert_eq!(
            ThumbnailHoverCursor::Default.unpolled_hover(),
            ThumbnailHoverCursor::Pointer
        );
        assert_eq!(
            ThumbnailHoverCursor::Pointer.unpolled_hover(),
            ThumbnailHoverCursor::Pointer
        );
        assert_eq!(
            ThumbnailHoverCursor::Grab.unpolled_hover(),
            ThumbnailHoverCursor::Grab
        );
        assert!(ThumbnailHoverCursor::Pointer.is_interactive());
        assert!(ThumbnailHoverCursor::Pointer.claims_ns_cursor());
        assert!(ThumbnailHoverCursor::Grab.claims_ns_cursor());
        assert!(!ThumbnailHoverCursor::Default.is_interactive());
        assert!(!ThumbnailHoverCursor::Default.claims_ns_cursor());
        assert!(!thumbnail_resets_cursor_on_exit());
        assert!(thumbnail_passthrough_disables_cursor_rects());
        assert!(thumbnail_css_fallback_restores_cursor_rects());
        assert!(should_restore_thumbnail_css_cursor_rects(true, false));
        assert!(!should_restore_thumbnail_css_cursor_rects(false, false));
        assert!(!should_restore_thumbnail_css_cursor_rects(true, true));
        assert!(!should_restore_thumbnail_css_cursor_rects(false, true));
        assert_eq!(
            thumbnail_unpolled_hover(false, ThumbnailHoverCursor::Default),
            ThumbnailHoverCursor::Pointer
        );
        assert_eq!(
            thumbnail_unpolled_hover(true, ThumbnailHoverCursor::Default),
            ThumbnailHoverCursor::Default
        );
        assert_eq!(
            thumbnail_unpolled_hover(false, ThumbnailHoverCursor::Grab),
            ThumbnailHoverCursor::Grab
        );
    }

    #[test]
    fn thumbnail_key_window_is_available_when_another_app_is_frontmost() {
        assert!(thumbnail_may_take_key_window(true, true));
        assert!(thumbnail_may_take_key_window(true, false));
        assert!(thumbnail_may_take_key_window(false, false));
        assert!(!thumbnail_may_take_key_window(false, true));
    }

    #[test]
    fn editor_cursor_rects_are_suppressed_while_a_preview_control_is_hovered() {
        assert!(suppress_document_cursor_rects_for_thumbnail(
            true, false, true
        ));
        assert!(!suppress_document_cursor_rects_for_thumbnail(
            true, true, true
        ));
        assert!(!suppress_document_cursor_rects_for_thumbnail(
            false, false, true
        ));
        assert!(!suppress_document_cursor_rects_for_thumbnail(
            true, false, false
        ));
    }
}
