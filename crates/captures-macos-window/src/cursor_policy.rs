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
}

/// Frozen JS poll only happens while Captures is inactive. Trust the hit-tested
/// kind once the app is active so empty stack chrome can keep the arrow.
#[must_use]
pub const fn thumbnail_unpolled_hover_when_inactive(
    app_is_active: bool,
    kind: ThumbnailHoverCursor,
) -> ThumbnailHoverCursor {
    if app_is_active {
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
        CaptureCursor, CaptureCursorKind, ThumbnailHoverCursor,
        overlay_prepare_keeps_native_cursor, region_shortcut_claims_cursor_on_press,
        suppress_document_cursor_rects_for_thumbnail, thumbnail_may_take_key_window,
        thumbnail_unpolled_hover_when_inactive,
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
    fn unpolled_thumbnail_hover_uses_a_pointing_hand() {
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
        assert!(!ThumbnailHoverCursor::Default.is_interactive());
        assert_eq!(
            thumbnail_unpolled_hover_when_inactive(false, ThumbnailHoverCursor::Default),
            ThumbnailHoverCursor::Pointer
        );
        assert_eq!(
            thumbnail_unpolled_hover_when_inactive(true, ThumbnailHoverCursor::Default),
            ThumbnailHoverCursor::Default
        );
        assert_eq!(
            thumbnail_unpolled_hover_when_inactive(false, ThumbnailHoverCursor::Grab),
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
