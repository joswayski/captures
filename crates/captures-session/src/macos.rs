use core_foundation::{
    base::TCFType,
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFDictionaryRef},
    string::CFString,
};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

pub(super) fn capture_session_available() -> bool {
    // CGSessionCopyCurrentDictionary returns null when this process has no GUI
    // session. The lock key is present and true only while loginwindow owns the
    // screen; the console key also rejects fast-user-switched sessions.
    let session = unsafe { CGSessionCopyCurrentDictionary() };
    if session.is_null() {
        return false;
    }
    let session = unsafe { CFDictionary::<CFString, CFBoolean>::wrap_under_create_rule(session) };
    session_allows_capture(
        session_boolean(&session, "kCGSSessionOnConsoleKey"),
        session_boolean(&session, "CGSSessionScreenIsLocked"),
    )
}

fn session_boolean(session: &CFDictionary<CFString, CFBoolean>, key: &str) -> Option<bool> {
    let key = CFString::new(key);
    session
        .find(&key)
        .map(|value| value.as_CFTypeRef() == CFBoolean::true_value().as_CFTypeRef())
}

fn session_allows_capture(on_console: Option<bool>, locked: Option<bool>) -> bool {
    on_console == Some(true) && locked != Some(true)
}

#[cfg(test)]
mod tests {
    use super::session_allows_capture;

    #[test]
    fn capture_requires_an_active_unlocked_console_session() {
        assert!(session_allows_capture(Some(true), Some(false)));
        assert!(session_allows_capture(Some(true), None));
        assert!(!session_allows_capture(Some(true), Some(true)));
        assert!(!session_allows_capture(Some(false), Some(false)));
        assert!(!session_allows_capture(None, None));
    }
}
