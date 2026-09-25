//! Read-only desktop motion preference. Call off the UI thread: portals can
//! take up to 250 ms to reply. No polling, subscription, or settings mutation.

/// `None` means unavailable, not an explicit preference for animation. AppKit
/// reads NSWorkspace directly; this adapter serves the Windows/Linux host.
pub fn prefers_reduced_motion() -> Option<bool> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
        };
        let mut enabled: windows_sys::core::BOOL = 1;
        // SAFETY: GET writes one BOOL to the valid, exclusively borrowed value.
        let success = unsafe {
            SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut enabled).cast(), 0)
        };
        (success != 0).then_some(enabled == 0)
    }
    #[cfg(target_os = "linux")]
    {
        use dbus::{
            arg::{RefArg, Variant},
            blocking::Connection,
        };
        use std::{collections::HashMap, time::Duration};
        type Settings = HashMap<String, HashMap<String, Variant<Box<dyn RefArg>>>>;
        let connection = Connection::new_session().ok()?;
        let proxy = connection.with_proxy(
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            Duration::from_millis(250),
        );
        // ReadAll is available on v1 and avoids Read's historical double variant.
        let (settings,): (Settings,) = proxy
            .method_call(
                "org.freedesktop.portal.Settings",
                "ReadAll",
                (vec!["org.freedesktop.appearance"],),
            )
            .ok()?;
        portal_value(
            settings
                .get("org.freedesktop.appearance")?
                .get("reduced-motion")?
                .0
                .as_ref(),
        )
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    None
}

#[cfg(target_os = "linux")]
fn portal_value(value: &dyn dbus::arg::RefArg) -> Option<bool> {
    // The standardized key is a uint32: 1 means reduced, unknown values mean no
    // preference. Do not interpret a malformed bool/string/signed integer as it.
    (value.arg_type() == dbus::arg::ArgType::UInt32)
        .then(|| value.as_u64().map(|value| value == 1))
        .flatten()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::portal_value;

    #[test]
    fn portal_values_follow_the_standard_without_coercing_other_types() {
        assert_eq!(portal_value(&0_u32), Some(false));
        assert_eq!(portal_value(&1_u32), Some(true));
        assert_eq!(portal_value(&2_u32), Some(false));
        assert_eq!(portal_value(&true), None);
        assert_eq!(portal_value(&1_i32), None);
        assert_eq!(portal_value(&"1".to_owned()), None);
    }
}
