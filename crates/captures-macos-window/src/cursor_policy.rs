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

    /// Frozen JS cannot prove the pointer is still on a live card. Drop to
    /// Default so leftover grab/pointer cannot keep the tall panel key over
    /// empty chrome and steal typing from apps underneath.
    #[must_use]
    pub const fn unpolled_hover(self) -> Self {
        let _ = self;
        Self::Default
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
/// WebKit timers are frozen, drop to Default instead of guessing.
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

/// A leftover cursor-claim panel can remain the key window after it is ordered
/// out. Keyboard then goes to Captures while mouse clicks reach the app
/// underneath. Resign whenever the panel is still key, even if it is hidden.
#[must_use]
pub const fn cursor_claim_panel_should_resign_key(is_key: bool) -> bool {
    is_key
}

/// Show the native cursor-claim panel only while a capture overlay is still
/// trying to become key.
///
/// The panel exists so a region overlay can own `NSCursor` before it is key.
/// The overlay window is created hidden at startup and reused; `NSCursor::set`
/// is ignored while Captures is inactive, so this panel must still appear for a
/// not-yet-presented overlay. After that overlay has been ordered front and
/// then hidden — including the JS fast-hide on pointer up — a queued reassert
/// must not resurrect it. A hidden nonactivating key panel swallows typing in
/// other apps until Captures quits.
///
/// `overlay_presented` is true only after this capture ordered the overlay
/// onscreen. Visibility alone cannot tell a precreated hidden overlay from a
/// dismissed one.
#[must_use]
pub const fn cursor_claim_panel_should_show(
    owns_cursor: bool,
    native_owned: bool,
    overlay_presented: bool,
    overlay_is_visible: bool,
    overlay_is_key: bool,
) -> bool {
    owns_cursor && native_owned && !overlay_is_key && (!overlay_presented || overlay_is_visible)
}

/// A delayed overlay/selector `makeKey` retry must not run after that surface
/// was dismissed, and must not `orderFront` a window that is already hidden.
#[must_use]
pub const fn capture_surface_focus_retry_allowed(
    scheduled_generation: u64,
    current_generation: u64,
    surface_is_visible: bool,
) -> bool {
    scheduled_generation == current_generation && surface_is_visible
}

/// macOS hardware key code for Escape (`kVK_Escape`).
pub const MACOS_ESCAPE_KEY_CODE: u16 = 53;

#[must_use]
pub const fn macos_key_code_is_escape(key_code: u16) -> bool {
    key_code == MACOS_ESCAPE_KEY_CODE
}

/// Native Escape monitors must fire even when the overlay is not key: another
/// screenshot tool can steal activation, or the freeze-frame may still be
/// painting.
///
/// Cursor ownership is not enough. A leftover claim after dismiss would keep
/// intercepting Escape in other apps. Arming and a still-visible overlay are
/// the only safe signals.
#[must_use]
pub const fn capture_escape_should_dispatch(
    armed: bool,
    overlay_visible: bool,
    overlay_owns_cursor: bool,
) -> bool {
    let _ = overlay_owns_cursor;
    armed || overlay_visible
}

/// A stale JS poll must not disable click-through just because the pointer is
/// somewhere in the (often tall) mini-preview frame.
#[must_use]
pub const fn thumbnail_stale_poll_may_disable_click_through() -> bool {
    false
}

/// Same for key-window status: becoming key over empty chrome swallows typing
/// in whichever app the panel happens to cover.
#[must_use]
pub const fn thumbnail_stale_poll_may_take_key_window() -> bool {
    false
}

/// Click-through mini-preview panels must drop key status. Clicks already reach
/// the app underneath; leftover key status is what steals typing.
#[must_use]
pub const fn thumbnail_passthrough_must_resign_key(click_through: bool) -> bool {
    click_through
}

/// After Captures resigns active, do not immediately reclaim key because the
/// pointer still sits in empty panel chrome covering the app the user clicked.
#[must_use]
pub const fn thumbnail_resign_active_may_retake_key() -> bool {
    false
}

/// A mouse click that AppKit delivered to another app (global monitor) must
/// release mini-preview key status so that app can receive typing.
#[must_use]
pub const fn thumbnail_foreign_mouse_click_must_resign_key() -> bool {
    true
}

/// Sleep/resume recovery must not force the whole stack hit-testable. The
/// preserved-height collapsed window would cover other apps until JS polls.
#[must_use]
pub const fn thumbnail_refresh_must_not_force_hit_testing() -> bool {
    true
}

/// Frozen JS also cannot keep leftover key status. Remaining key over empty
/// chrome is what lets clicks reach another app while typing stays in Captures.
#[must_use]
pub const fn thumbnail_stale_poll_must_resign_key() -> bool {
    true
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
        MACOS_ESCAPE_KEY_CODE, ThumbnailHoverCursor, capture_cursor_monitor_action,
        capture_escape_should_dispatch, capture_surface_focus_retry_allowed,
        cursor_claim_panel_should_resign_key, cursor_claim_panel_should_show,
        macos_key_code_is_escape, overlay_prepare_keeps_native_cursor,
        region_shortcut_claims_cursor_on_press, suppress_document_cursor_rects_for_thumbnail,
        thumbnail_foreign_mouse_click_must_resign_key, thumbnail_may_take_key_window,
        thumbnail_passthrough_disables_cursor_rects, thumbnail_passthrough_must_resign_key,
        thumbnail_poll_is_live, thumbnail_refresh_must_not_force_hit_testing,
        thumbnail_resets_cursor_on_exit, thumbnail_resign_active_may_retake_key,
        thumbnail_stale_poll_may_disable_click_through, thumbnail_stale_poll_may_take_key_window,
        thumbnail_stale_poll_must_resign_key, thumbnail_unpolled_hover,
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
    fn hidden_cursor_claim_panels_still_resign_key() {
        assert!(cursor_claim_panel_should_resign_key(true));
        assert!(!cursor_claim_panel_should_resign_key(false));
    }

    #[test]
    fn cursor_claim_panel_is_not_resurrected_after_the_overlay_hides() {
        // Overlay not created yet, or precreated at startup and still hidden:
        // region shortcuts claim the cursor before present.
        assert!(cursor_claim_panel_should_show(
            true, true, false, false, false
        ));
        // Primed overlay is on-screen but not yet key.
        assert!(cursor_claim_panel_should_show(
            true, true, true, true, false
        ));
        // Overlay is key: drop the claim panel.
        assert!(!cursor_claim_panel_should_show(
            true, true, true, true, true
        ));
        // JS/Tauri hide ordered the overlay out after it was presented, while
        // cursor ownership is still live. A queued reassert must not make the
        // claim panel key.
        assert!(!cursor_claim_panel_should_show(
            true, true, true, false, false
        ));
        // Window capture does not use the native claim panel.
        assert!(!cursor_claim_panel_should_show(
            true, false, false, false, false
        ));
        assert!(!cursor_claim_panel_should_show(
            false, true, false, false, false
        ));
    }

    #[test]
    fn precreated_hidden_overlay_still_claims_the_region_crosshair() {
        // `create_overlay_window` configures the overlay and leaves it hidden.
        // Existence/visibility must not be treated as "already dismissed."
        assert!(cursor_claim_panel_should_show(
            true, true, false, false, false
        ));
        assert!(!cursor_claim_panel_should_show(
            true, true, true, false, false
        ));
    }

    #[test]
    fn dismissed_capture_surfaces_do_not_retry_make_key() {
        assert!(capture_surface_focus_retry_allowed(3, 3, true));
        assert!(!capture_surface_focus_retry_allowed(3, 4, true));
        assert!(!capture_surface_focus_retry_allowed(3, 3, false));
    }

    #[test]
    fn escape_cancels_when_another_tool_stole_key_focus() {
        assert!(macos_key_code_is_escape(MACOS_ESCAPE_KEY_CODE));
        assert!(!macos_key_code_is_escape(0));
        assert!(capture_escape_should_dispatch(true, false, false));
        assert!(capture_escape_should_dispatch(false, true, false));
        // Leftover cursor ownership after dismiss must not keep intercepting
        // Escape in other apps.
        assert!(!capture_escape_should_dispatch(false, false, true));
        assert!(!capture_escape_should_dispatch(false, false, false));
    }

    #[test]
    fn thumbnail_poll_liveness_has_a_250ms_stale_threshold() {
        assert!(thumbnail_poll_is_live(0));
        assert!(thumbnail_poll_is_live(250));
        assert!(!thumbnail_poll_is_live(251));
    }

    #[test]
    fn unpolled_thumbnail_hover_does_not_promote_empty_chrome() {
        assert_eq!(
            ThumbnailHoverCursor::Default.unpolled_hover(),
            ThumbnailHoverCursor::Default
        );
        assert_eq!(
            ThumbnailHoverCursor::Pointer.unpolled_hover(),
            ThumbnailHoverCursor::Default
        );
        assert_eq!(
            ThumbnailHoverCursor::Grab.unpolled_hover(),
            ThumbnailHoverCursor::Default
        );
        assert!(ThumbnailHoverCursor::Pointer.is_interactive());
        assert!(ThumbnailHoverCursor::Pointer.claims_ns_cursor());
        assert!(ThumbnailHoverCursor::Grab.claims_ns_cursor());
        assert!(!ThumbnailHoverCursor::Default.is_interactive());
        assert!(!ThumbnailHoverCursor::Default.claims_ns_cursor());
        assert!(!thumbnail_resets_cursor_on_exit());
        assert!(thumbnail_passthrough_disables_cursor_rects());
        assert_eq!(
            thumbnail_unpolled_hover(false, ThumbnailHoverCursor::Default),
            ThumbnailHoverCursor::Default
        );
        assert_eq!(
            thumbnail_unpolled_hover(true, ThumbnailHoverCursor::Default),
            ThumbnailHoverCursor::Default
        );
        assert_eq!(
            thumbnail_unpolled_hover(false, ThumbnailHoverCursor::Grab),
            ThumbnailHoverCursor::Default
        );
        assert_eq!(
            thumbnail_unpolled_hover(true, ThumbnailHoverCursor::Grab),
            ThumbnailHoverCursor::Grab
        );
    }

    #[test]
    fn stale_thumbnail_frame_hover_must_not_steal_desktop_input() {
        assert!(!thumbnail_stale_poll_may_disable_click_through());
        assert!(!thumbnail_stale_poll_may_take_key_window());
        assert!(thumbnail_passthrough_must_resign_key(true));
        assert!(!thumbnail_passthrough_must_resign_key(false));
        assert!(!thumbnail_resign_active_may_retake_key());
        assert!(thumbnail_foreign_mouse_click_must_resign_key());
        assert!(thumbnail_refresh_must_not_force_hit_testing());
        assert!(thumbnail_stale_poll_must_resign_key());
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
