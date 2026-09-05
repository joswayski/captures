//! When capture UI may order out titled documents, and when the update notice
//! must push those documents (editors, history, preferences) back behind the
//! user's work.

/// Whether titled documents should be ordered out for this capture activation.
///
/// `other_app_is_frontmost` is true when the workspace frontmost app is not
/// Captures (and is still running). `captures_holds_user_focus` is true when
/// Captures is active or a titled document (editor, history, preferences) is
/// key — including LSUIElement cases where NSWorkspace still reports another
/// app as frontmost.
pub fn should_conceal_documents_for_capture_activation(
    other_app_is_frontmost: bool,
    captures_holds_user_focus: bool,
) -> bool {
    other_app_is_frontmost && !captures_holds_user_focus
}

/// Whether Later/Close should push a donated key window behind the user's app.
///
/// Hiding the update notice donates key status to Preferences, history, or an
/// editor. Resigning that window is not enough: AppKit has already raised it
/// in the screen list, so it stays on top until activation yield completes.
/// Order it back first, matching mini-preview panel dismissal.
pub fn should_order_donated_document_behind_after_notice_dismiss(
    handing_off_to_external_app: bool,
    donated_key_is_titled_document: bool,
) -> bool {
    handing_off_to_external_app && donated_key_is_titled_document
}

#[cfg(test)]
mod tests {
    use super::{
        should_conceal_documents_for_capture_activation,
        should_order_donated_document_behind_after_notice_dismiss,
    };

    #[test]
    fn conceals_documents_only_when_another_app_is_frontmost() {
        // Editor open + capture shortcut: Captures is already key — keep the
        // editor on screen for the whole selection session.
        assert!(!should_conceal_documents_for_capture_activation(
            false, false
        ));
        assert!(!should_conceal_documents_for_capture_activation(
            false, true
        ));
        // Workspace may still report Chrome while an editor is key (LSUIElement).
        assert!(!should_conceal_documents_for_capture_activation(true, true));
        // Capture while Chrome/Discord is key: order out so activation cannot
        // flash editors above the user's work when the overlay dismisses.
        assert!(should_conceal_documents_for_capture_activation(true, false));
    }

    #[test]
    fn later_orders_a_donated_preferences_window_behind_the_users_app() {
        // Preferences is open behind a browser; Later must not leave it
        // visually in front while activation yields back to that browser.
        assert!(should_order_donated_document_behind_after_notice_dismiss(
            true, true
        ));
        assert!(!should_order_donated_document_behind_after_notice_dismiss(
            true, false
        ));
        // User was already working in Captures: keep the donated document key.
        assert!(!should_order_donated_document_behind_after_notice_dismiss(
            false, true
        ));
    }
}
