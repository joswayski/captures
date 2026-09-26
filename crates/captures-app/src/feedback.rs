//! Feedback form copy, limits and presentation policy.
//!
//! Ported from the shipping Tauri surface: `apps/desktop/ui/src/Feedback.tsx`
//! and its `.feedback*` rules in `apps/desktop/ui/src/styles/windows.css`, plus
//! the `show_feedback` window in `apps/desktop/src-tauri/src/feedback.rs`.
//! Both native hosts read this module (AppKit through
//! `captures_feedback_request_v1`'s `copy` operation) so their copy stays
//! identical. Sending itself lives in `captures-feedback`.
use serde::Serialize;
use serde_json::{Value, json};

/// Shipping window: its own titled, resizable window.
pub const WINDOW_TITLE: &str = "Send Feedback";
pub const WINDOW_WIDTH: f32 = 640.0;
pub const WINDOW_HEIGHT: f32 = 700.0;
pub const WINDOW_MIN_WIDTH: f32 = 460.0;
pub const WINDOW_MIN_HEIGHT: f32 = 460.0;
/// `.feedback-shell { max-width: 640px }`.
pub const SHELL_MAX_WIDTH: f32 = 640.0;

pub const EYEBROW: &str = "Captures";
pub const TITLE: &str = "Send feedback";
pub const INTRO: &str = "Tell us what broke, what is missing, or what you wish worked better. Captures sends what you type here plus the app and system details listed below.";

pub const CATEGORY_LABEL: &str = "Category";
pub const MESSAGE_LABEL: &str = "Message";
pub const CONTACT_LABEL: &str = "Contact";
pub const OPTIONAL_BADGE: &str = "optional";
pub const CONTACT_PLACEHOLDER: &str = "X handle, GitHub username, email…";
pub const CONTACT_HELP: &str = "Optional — we may use this if we need to ask a follow-up question.";

pub const META_TITLE: &str = "Included automatically";
pub const APP_VERSION_LABEL: &str = "App version";
pub const SYSTEM_LABEL: &str = "System";
/// Shipping shows this for each detail until the context arrives.
pub const LOADING: &str = "…";

pub const SEND: &str = "Send feedback";
pub const SENDING: &str = "Sending…";
pub const SENT: &str = "Thanks — feedback sent.";

/// Native-only states shipping cannot reach (fixtures never send, and a
/// failed local context read blocks Send until it is retried).
pub const FIXTURE_DISABLED: &str = "Fixture mode — sending feedback is disabled.";
pub const CONTEXT_ERROR: &str = "App and system details could not be loaded.";
pub const RETRY_CONTEXT: &str = "Retry details";

/// `maxLength` on the message and contact fields; `captures-feedback`
/// rejects anything longer, counted in Unicode scalar values.
pub const MESSAGE_LIMIT: usize = 8_000;
pub const CONTACT_LIMIT: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Category {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub placeholder: &'static str,
}

/// Shipping `CATEGORIES` and `MESSAGE_PLACEHOLDERS`, in display order.
/// The first entry is the default.
pub const CATEGORIES: [Category; 3] = [
    Category {
        id: "bug",
        label: "Bug",
        description: "Something is broken or unexpected",
        placeholder: "What happened? What did you expect?",
    },
    Category {
        id: "idea",
        label: "Idea",
        description: "A feature or improvement",
        placeholder: "What's the idea? What problem would it solve?",
    },
    Category {
        id: "other",
        label: "Other",
        description: "Anything else",
        placeholder: "What would you like us to know?",
    },
];

/// Shipping `formatOsLabel`: the non-empty parts joined with " · ".
pub fn system_label(os: &str, os_version: &str, arch: &str) -> String {
    [os, os_version, arch]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Shipping `canSubmit` plus the field limits the backend enforces.
pub fn can_submit(message: &str, contact: &str, sending: bool) -> bool {
    !sending
        && !message.trim().is_empty()
        && message.chars().count() <= MESSAGE_LIMIT
        && contact.chars().count() <= CONTACT_LIMIT
}

/// Truncate `text` to at most `limit` Unicode scalar values, like `maxLength`.
/// Returns true when anything was removed.
pub fn clamp(text: &mut String, limit: usize) -> bool {
    match text.char_indices().nth(limit) {
        Some((index, _)) => {
            text.truncate(index);
            true
        }
        None => false,
    }
}

/// All presentation copy and limits for the AppKit host.
pub fn copy() -> Value {
    json!({
        "window_title": WINDOW_TITLE,
        "window_width": WINDOW_WIDTH,
        "window_height": WINDOW_HEIGHT,
        "window_min_width": WINDOW_MIN_WIDTH,
        "window_min_height": WINDOW_MIN_HEIGHT,
        "shell_max_width": SHELL_MAX_WIDTH,
        "eyebrow": EYEBROW,
        "title": TITLE,
        "intro": INTRO,
        "category_label": CATEGORY_LABEL,
        "categories": CATEGORIES,
        "message_label": MESSAGE_LABEL,
        "contact_label": CONTACT_LABEL,
        "optional_badge": OPTIONAL_BADGE,
        "contact_placeholder": CONTACT_PLACEHOLDER,
        "contact_help": CONTACT_HELP,
        "meta_title": META_TITLE,
        "app_version_label": APP_VERSION_LABEL,
        "system_label": SYSTEM_LABEL,
        "loading": LOADING,
        "send": SEND,
        "sending": SENDING,
        "sent": SENT,
        "fixture_disabled": FIXTURE_DISABLED,
        "context_error": CONTEXT_ERROR,
        "retry_context": RETRY_CONTEXT,
        "message_limit": MESSAGE_LIMIT,
        "contact_limit": CONTACT_LIMIT,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_matches_shipping_feedback_tsx() {
        assert!(INTRO.ends_with(
            "Captures sends what you type here plus the app and system details listed below."
        ));
        assert_eq!(CATEGORIES.map(|c| c.id), ["bug", "idea", "other"]);
        assert_eq!(
            CATEGORIES[1].placeholder,
            "What's the idea? What problem would it solve?"
        );
        assert_eq!(
            CATEGORIES[0].description,
            "Something is broken or unexpected"
        );
        let value = copy();
        assert_eq!(
            value["categories"][2]["placeholder"],
            CATEGORIES[2].placeholder
        );
        assert_eq!(value["message_limit"], 8_000);
        assert_eq!(value["contact_help"], CONTACT_HELP);
        assert_eq!(value["loading"], "…");
    }

    #[test]
    fn system_label_skips_empty_parts() {
        assert_eq!(
            system_label("macos", "15.5", "aarch64"),
            "macos · 15.5 · aarch64"
        );
        assert_eq!(system_label("windows", "", "x86_64"), "windows · x86_64");
        assert_eq!(system_label("", "", ""), "");
    }

    #[test]
    fn submit_policy_and_clamp_count_unicode_scalars() {
        assert!(!can_submit("   ", "", false));
        assert!(!can_submit("hi", "", true));
        assert!(can_submit(&"🦀".repeat(8_000), &"é".repeat(200), false));
        assert!(!can_submit(&"🦀".repeat(8_001), "", false));
        assert!(!can_submit("hi", &"é".repeat(201), false));
        let mut text = "🦀".repeat(8_003);
        assert!(clamp(&mut text, MESSAGE_LIMIT));
        assert_eq!(text.chars().count(), 8_000);
        assert!(!clamp(&mut text, MESSAGE_LIMIT));
    }
}
