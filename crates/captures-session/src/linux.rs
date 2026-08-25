use std::time::Duration;

use dbus::{Path, blocking::Connection, blocking::stdintf::org_freedesktop_dbus::Properties};

const QUERY_TIMEOUT: Duration = Duration::from_millis(250);
const LOGIN1_DESTINATION: &str = "org.freedesktop.login1";
const LOGIN1_MANAGER_PATH: &str = "/org/freedesktop/login1";
const LOGIN1_MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const LOGIN1_SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";
const DBUS_DESTINATION: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";
const SCREEN_SAVER_SERVICES: [(&str, &str, &str); 5] = [
    (
        "org.freedesktop.ScreenSaver",
        "/org/freedesktop/ScreenSaver",
        "org.freedesktop.ScreenSaver",
    ),
    (
        "org.gnome.ScreenSaver",
        "/org/gnome/ScreenSaver",
        "org.gnome.ScreenSaver",
    ),
    (
        "org.cinnamon.ScreenSaver",
        "/org/cinnamon/ScreenSaver",
        "org.cinnamon.ScreenSaver",
    ),
    (
        "org.mate.ScreenSaver",
        "/org/mate/ScreenSaver",
        "org.mate.ScreenSaver",
    ),
    (
        "org.xfce.ScreenSaver",
        "/org/xfce/ScreenSaver",
        "org.xfce.ScreenSaver",
    ),
];

pub(super) fn capture_session_available() -> bool {
    session_allows_capture(login1_session_state(), screen_saver_locked())
}

fn login1_session_state() -> Option<(bool, bool)> {
    let connection = Connection::new_system().ok()?;
    let manager = connection.with_proxy(LOGIN1_DESTINATION, LOGIN1_MANAGER_PATH, QUERY_TIMEOUT);
    let session_by_id: Option<(Path<'static>,)> =
        std::env::var("XDG_SESSION_ID").ok().and_then(|session_id| {
            manager
                .method_call(LOGIN1_MANAGER_INTERFACE, "GetSession", (session_id,))
                .ok()
        });
    let (session_path,) = session_by_id.or_else(|| {
        manager
            .method_call(
                LOGIN1_MANAGER_INTERFACE,
                "GetSessionByPID",
                (std::process::id(),),
            )
            .ok()
    })?;
    let session = connection.with_proxy(LOGIN1_DESTINATION, session_path, QUERY_TIMEOUT);
    let active = session.get(LOGIN1_SESSION_INTERFACE, "Active").ok()?;
    let locked = session.get(LOGIN1_SESSION_INTERFACE, "LockedHint").ok()?;
    Some((active, locked))
}

fn screen_saver_locked() -> Option<bool> {
    let connection = Connection::new_session().ok()?;
    SCREEN_SAVER_SERVICES
        .iter()
        .find_map(|(name, path, interface)| {
            let bus = connection.with_proxy(DBUS_DESTINATION, DBUS_PATH, QUERY_TIMEOUT);
            let (has_owner,): (bool,) = bus
                .method_call(DBUS_INTERFACE, "NameHasOwner", (*name,))
                .ok()?;
            if !has_owner {
                return None;
            }
            let proxy = connection.with_proxy(*name, *path, QUERY_TIMEOUT);
            let (active,): (bool,) = proxy.method_call(*interface, "GetActive", ()).ok()?;
            Some(active)
        })
}

fn session_allows_capture(
    login1_state: Option<(bool, bool)>,
    screen_saver_locked: Option<bool>,
) -> bool {
    if login1_state.is_some_and(|(active, locked)| !active || locked)
        || screen_saver_locked == Some(true)
    {
        return false;
    }

    login1_state.is_some() || screen_saver_locked.is_some()
}

#[cfg(test)]
mod tests {
    use super::session_allows_capture;

    #[test]
    fn capture_requires_a_known_active_unlocked_session() {
        assert!(session_allows_capture(Some((true, false)), None));
        assert!(session_allows_capture(None, Some(false)));
        assert!(session_allows_capture(Some((true, false)), Some(false)));
        assert!(!session_allows_capture(Some((false, false)), Some(false)));
        assert!(!session_allows_capture(Some((true, true)), Some(false)));
        assert!(!session_allows_capture(Some((true, false)), Some(true)));
        assert!(!session_allows_capture(None, None));
    }
}
