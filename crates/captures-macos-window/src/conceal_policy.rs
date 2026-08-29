//! When capture UI may order out titled documents (editors, history, preferences).

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

#[cfg(test)]
mod tests {
    use super::should_conceal_documents_for_capture_activation;

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
}
