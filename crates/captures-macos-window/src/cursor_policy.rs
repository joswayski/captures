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

#[cfg(test)]
mod tests {
    use super::{CaptureCursor, CaptureCursorKind};

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
}
