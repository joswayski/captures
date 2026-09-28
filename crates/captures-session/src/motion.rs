//! Read-only desktop motion preference. Call off the UI thread: portals can
//! take up to 250 ms to reply. [`watch_reduced_motion`] subscribes to change
//! notifications on its own thread; neither polls nor mutates settings.

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

/// Calls `changed` on a background thread whenever the desktop reports a new
/// motion preference, with the same meaning as [`prefers_reduced_motion`].
/// Windows listens for `WM_SETTINGCHANGE` (`SPI_SETCLIENTAREAANIMATION`) on a
/// hidden top-level window, since broadcasts skip message-only windows; Linux
/// listens for the Settings portal's `SettingChanged` on
/// `org.freedesktop.appearance` / `reduced-motion`. Returns false when no
/// notification source exists (another OS, no session bus, no window), leaving
/// hosts to re-read on their own triggers. The watcher lives for the process.
pub fn watch_reduced_motion(changed: impl Fn(Option<bool>) + Send + 'static) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_watch::start(Box::new(changed))
    }
    #[cfg(target_os = "linux")]
    {
        portal_watch(Box::new(changed))
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        drop(changed);
        false
    }
}

#[cfg(target_os = "linux")]
fn portal_watch(changed: Box<dyn Fn(Option<bool>) + Send>) -> bool {
    use dbus::{
        arg::{RefArg, Variant},
        blocking::Connection,
        message::MatchRule,
    };
    use std::{sync::mpsc, time::Duration};
    let (ready, subscribed) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("captures-motion-watch".into())
        .spawn(move || {
            let Ok(connection) = Connection::new_session() else {
                let _ = ready.send(false);
                return;
            };
            let rule = MatchRule::new_signal("org.freedesktop.portal.Settings", "SettingChanged")
                .with_path("/org/freedesktop/portal/desktop");
            let subscription = connection.add_match(
                rule,
                move |(namespace, key, value): (String, String, Variant<Box<dyn RefArg>>),
                      _: &Connection,
                      _: &dbus::Message| {
                    if namespace == "org.freedesktop.appearance" && key == "reduced-motion" {
                        changed(portal_value(value.0.as_ref()));
                    }
                    true
                },
            );
            let _ = ready.send(subscription.is_ok());
            if subscription.is_err() {
                return;
            }
            // Blocks until a signal arrives; ends with the session bus.
            while connection.process(Duration::from_secs(3600)).is_ok() {}
        })
        .is_ok();
    spawned && subscribed.recv().unwrap_or(false)
}

#[cfg(target_os = "windows")]
mod windows_watch {
    use std::{cell::RefCell, sync::mpsc};
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassW,
            SPI_SETCLIENTAREAANIMATION, WM_SETTINGCHANGE, WNDCLASSW, WS_OVERLAPPED,
        },
    };

    type Changed = Box<dyn Fn(Option<bool>) + Send>;

    thread_local! {
        static CHANGED: RefCell<Option<Changed>> = const { RefCell::new(None) };
    }

    unsafe extern "system" fn procedure(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_SETTINGCHANGE && wparam == SPI_SETCLIENTAREAANIMATION as WPARAM {
            let value = super::prefers_reduced_motion();
            CHANGED.with(|changed| {
                if let Some(changed) = changed.borrow().as_ref() {
                    changed(value);
                }
            });
        }
        // SAFETY: forwards this window's own message unchanged.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    pub(super) fn start(changed: Changed) -> bool {
        let (ready, created) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("captures-motion-watch".into())
            .spawn(move || {
                CHANGED.with(|slot| *slot.borrow_mut() = Some(changed));
                let class: Vec<u16> = "CapturesMotionWatch\0".encode_utf16().collect();
                // SAFETY: a null name returns this executable's module handle.
                let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
                let definition = WNDCLASSW {
                    lpfnWndProc: Some(procedure),
                    hInstance: instance,
                    lpszClassName: class.as_ptr(),
                    ..Default::default()
                };
                // SAFETY: the class name outlives registration and the window;
                // `procedure` matches WNDPROC.
                let registered = unsafe { RegisterClassW(&definition) } != 0;
                // A hidden (never shown) top-level window: WM_SETTINGCHANGE is
                // broadcast to top-level windows only.
                // SAFETY: all pointers are valid or null as CreateWindowExW allows.
                let window = registered.then(|| unsafe {
                    CreateWindowExW(
                        0,
                        class.as_ptr(),
                        class.as_ptr(),
                        WS_OVERLAPPED,
                        0,
                        0,
                        0,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        instance,
                        std::ptr::null(),
                    )
                });
                let ok = window.is_some_and(|window| !window.is_null());
                let _ = ready.send(ok);
                if !ok {
                    return;
                }
                let mut message = MSG::default();
                // SAFETY: `message` is a valid, exclusively borrowed MSG; this
                // thread owns the window whose messages it pumps.
                while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
                    unsafe { DispatchMessageW(&message) };
                }
            })
            .is_ok();
        spawned && created.recv().unwrap_or(false)
    }
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
