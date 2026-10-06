//! GlobalShortcuts transport, confined to a worker and its portal-owned session.
use super::{Binding, Bindings, CaptureShortcut, Dispatcher, PortalShortcutStatus};
use dbus::{
    Path,
    arg::{PropMap, RefArg, Variant},
    blocking::{Connection, stdintf::org_freedesktop_dbus::Properties},
    message::MatchRule,
};
use global_hotkey::{
    GlobalHotKeyEvent, HotKeyState,
    hotkey::{HotKey, Modifiers},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const DESKTOP: &str = "org.freedesktop.portal.Desktop";
const ROOT: &str = "/org/freedesktop/portal/desktop";
const GLOBAL: &str = "org.freedesktop.portal.GlobalShortcuts";
const REQUEST: &str = "org.freedesktop.portal.Request";
const SESSION: &str = "org.freedesktop.portal.Session";
const CALL: Duration = Duration::from_secs(1);
const CONSENT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(50);
type Response = (String, Result<(u32, PropMap), String>);

pub(super) struct Worker {
    status: Arc<Mutex<PortalShortcutStatus>>,
    cancelled: Arc<AtomicBool>,
    commands: SyncSender<()>,
    worker: Option<JoinHandle<()>>,
}

impl Worker {
    pub(super) fn start(desired: Bindings, dispatcher: Arc<Dispatcher>) -> Self {
        let status = Arc::new(Mutex::new(PortalShortcutStatus::Pending));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (commands, incoming) = mpsc::sync_channel(1);
        let worker_status = status.clone();
        let worker_cancelled = cancelled.clone();
        let worker = thread::spawn(move || {
            let result = run(
                &desired,
                &dispatcher,
                &worker_status,
                &worker_cancelled,
                incoming,
            );
            if let Err(error) = result {
                revoke(&dispatcher);
                if !worker_cancelled.load(Ordering::Acquire) {
                    let mut status = worker_status.lock().unwrap();
                    if !matches!(*status, PortalShortcutStatus::Unavailable(_)) {
                        *status = PortalShortcutStatus::Unavailable(error);
                    }
                    drop(status);
                    (dispatcher.wake)();
                }
            }
        });
        Self {
            status,
            cancelled,
            commands,
            worker: Some(worker),
        }
    }

    pub(super) fn status(&self) -> PortalShortcutStatus {
        self.status.lock().unwrap().clone()
    }

    pub(super) fn configure(&self) -> Result<(), String> {
        if !matches!(
            self.status(),
            PortalShortcutStatus::Bound {
                configurable: true,
                ..
            }
        ) {
            return Err(
                "This desktop portal does not support in-app shortcut configuration".into(),
            );
        }
        match self.commands.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => Ok(()),
            Err(TrySendError::Disconnected(())) => {
                Err("The desktop shortcut session has ended".into())
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Session {
    connection: Connection,
    owner: String,
    path: Path<'static>,
    responses: Receiver<Response>,
    closed: Arc<AtomicBool>,
    changed: Arc<AtomicBool>,
}

fn run(
    desired: &Bindings,
    dispatcher: &Arc<Dispatcher>,
    status: &Arc<Mutex<PortalShortcutStatus>>,
    cancelled: &AtomicBool,
    commands: Receiver<()>,
) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    let session = Session::open(desired, dispatcher, status)?;
    session.pending(cancelled)?;
    let version: u32 = session
        .connection
        .with_proxy(&session.owner, ROOT, CALL)
        .get(GLOBAL, "version")
        .map_err(error)?;
    if version == 0 {
        return Err(error("the desktop does not provide global shortcuts"));
    }
    let session_token = session.path.rsplit('/').next().unwrap().to_owned();
    let results = session.request("CreateSession", cancelled, |proxy, mut options| {
        options.insert("session_handle_token".into(), value(session_token));
        proxy.method_call(GLOBAL, "CreateSession", (options,))
    })?;
    if results.get("session_handle").and_then(|v| v.0.as_str()) != Some(session.path.as_ref()) {
        return Err(error("the portal returned a foreign session handle"));
    }
    let shortcuts: Vec<(String, PropMap)> = desired
        .values()
        .map(|binding| {
            let mut properties = PropMap::new();
            properties.insert(
                "description".into(),
                value(description(binding.action).to_owned()),
            );
            if let Some(trigger) = preferred_trigger(binding.key) {
                properties.insert("preferred_trigger".into(), value(trigger));
            }
            (binding.action.id().to_owned(), properties)
        })
        .collect();
    let path = session.path.clone();
    let results = session.request("BindShortcuts", cancelled, |proxy, options| {
        proxy.method_call(GLOBAL, "BindShortcuts", (path, shortcuts, "", options))
    })?;
    session.pending(cancelled)?;
    let (bindings, triggers) = bound_shortcuts(&results, desired)?;
    publish(bindings, triggers, version >= 2, dispatcher, status);
    while !cancelled.load(Ordering::Acquire) {
        session.pending(cancelled)?;
        if session.changed.swap(false, Ordering::AcqRel) {
            // ShortcutsChanged may contain only changed entries. Reconcile
            // against ListShortcuts rather than inventing removals or keeping
            // revoked bindings. The signal already cleared held/queued events.
            let results = session.request("ListShortcuts", cancelled, |proxy, options| {
                proxy.method_call(GLOBAL, "ListShortcuts", (session.path.clone(), options))
            })?;
            session.pending(cancelled)?;
            let (bindings, triggers) = bound_shortcuts(&results, desired)?;
            if !session.changed.load(Ordering::Acquire) {
                publish(bindings, triggers, version >= 2, dispatcher, status);
            }
        }
        if commands.try_recv().is_ok() {
            let result: Result<(), dbus::Error> = session
                .connection
                .with_proxy(&session.owner, ROOT, CALL)
                .method_call(
                    GLOBAL,
                    "ConfigureShortcuts",
                    (session.path.clone(), "", PropMap::new()),
                );
            if let PortalShortcutStatus::Bound {
                configuration_error,
                ..
            } = &mut *status.lock().unwrap()
            {
                *configuration_error = result.err().map(error);
            }
            (dispatcher.wake)();
        }
        session.connection.process(POLL).map_err(error)?;
    }
    Ok(())
}

impl Session {
    fn open(
        desired: &Bindings,
        dispatcher: &Arc<Dispatcher>,
        status: &Arc<Mutex<PortalShortcutStatus>>,
    ) -> Result<Self, String> {
        let connection = Connection::new_session().map_err(error)?;
        let bus = connection.with_proxy("org.freedesktop.DBus", "/org/freedesktop/DBus", CALL);
        // A desktop may activate the public portal only on first use. Start it
        // before pinning its unique owner, as the ScreenCast adapter does.
        let _: (u32,) = bus
            .method_call(
                "org.freedesktop.DBus",
                "StartServiceByName",
                (DESKTOP, 0_u32),
            )
            .map_err(error)?;
        let (owner,): (String,) = bus
            .method_call("org.freedesktop.DBus", "GetNameOwner", (DESKTOP,))
            .map_err(error)?;
        let sender = connection
            .unique_name()
            .to_string()
            .trim_start_matches(':')
            .replace('.', "_");
        let path = Path::new(format!("{ROOT}/session/{sender}/{}", token()))
            .map_err(error)?
            .into_static();
        let closed = Arc::new(AtomicBool::new(false));
        let (out, responses) = mpsc::channel();
        let pinned = owner.clone();
        connection
            .add_match(
                MatchRule::new_signal(REQUEST, "Response").with_sender(DESKTOP),
                move |_: (), _, message| {
                    if message
                        .sender()
                        .is_some_and(|sender| sender.as_ref() == pinned)
                        && let Some(path) = message.path()
                    {
                        let _ = out.send((
                            path.to_string(),
                            message.read2::<u32, PropMap>().map_err(error),
                        ));
                    }
                    true
                },
            )
            .map_err(error)?;
        for (member, phase) in [
            ("Activated", HotKeyState::Pressed),
            ("Deactivated", HotKeyState::Released),
        ] {
            let owner = owner.clone();
            let path = path.clone();
            let desired = desired.clone();
            let dispatcher = dispatcher.clone();
            let closed = closed.clone();
            connection
                .add_match(
                    MatchRule::new_signal(GLOBAL, member).with_sender(DESKTOP),
                    move |_: (), _, message| {
                        if !ours(message, &owner, ROOT) || closed.load(Ordering::Acquire) {
                            return true;
                        }
                        if let Ok((session, id, _, _)) =
                            message.read4::<Path<'static>, String, u64, PropMap>()
                            && session == path
                            && let Some(binding) = desired.values().find(|b| b.action.id() == id)
                        {
                            dispatcher.event(GlobalHotKeyEvent {
                                id: binding.key.id(),
                                state: phase,
                            });
                        }
                        true
                    },
                )
                .map_err(error)?;
        }
        let pinned = owner.clone();
        let expected = path.to_string();
        let stopped = closed.clone();
        let closed_dispatcher = dispatcher.clone();
        connection
            .add_match(
                MatchRule::new_signal(SESSION, "Closed").with_sender(DESKTOP),
                move |_: (), _, message| {
                    if ours(message, &pinned, &expected) {
                        stopped.store(true, Ordering::Release);
                        revoke(&closed_dispatcher);
                    }
                    true
                },
            )
            .map_err(error)?;
        let pinned = owner.clone();
        let stopped = closed.clone();
        let lost_dispatcher = dispatcher.clone();
        connection
            .add_match(
                MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged")
                    .with_sender("org.freedesktop.DBus"),
                move |_: (), _, message| {
                    if message
                        .sender()
                        .is_some_and(|sender| sender == "org.freedesktop.DBus")
                        && let Ok((name, previous, next)) =
                            message.read3::<String, String, String>()
                        && name == DESKTOP
                        && previous == pinned
                        && next != pinned
                    {
                        stopped.store(true, Ordering::Release);
                        revoke(&lost_dispatcher);
                    }
                    true
                },
            )
            .map_err(error)?;
        let dispatcher = dispatcher.clone();
        let status = status.clone();
        let pinned = owner.clone();
        let expected = path.clone();
        let stopped = closed.clone();
        let changed = Arc::new(AtomicBool::new(false));
        let refresh = changed.clone();
        connection
            .add_match(
                MatchRule::new_signal(GLOBAL, "ShortcutsChanged").with_sender(DESKTOP),
                move |_: (), _, message| {
                    if !ours(message, &pinned, ROOT)
                        || stopped.load(Ordering::Acquire)
                        || !matches!(*status.lock().unwrap(), PortalShortcutStatus::Bound { .. })
                    {
                        return true;
                    }
                    if let Ok((path, _)) = message.read2::<Path<'static>, Vec<(String, PropMap)>>()
                        && path == expected
                    {
                        revoke(&dispatcher);
                        refresh.store(true, Ordering::Release);
                    }
                    true
                },
            )
            .map_err(error)?;
        Ok(Self {
            connection,
            owner,
            path,
            responses,
            closed,
            changed,
        })
    }

    fn request(
        &self,
        method: &str,
        cancelled: &AtomicBool,
        call: impl FnOnce(
            dbus::blocking::Proxy<'_, &Connection>,
            PropMap,
        ) -> Result<(Path<'static>,), dbus::Error>,
    ) -> Result<PropMap, String> {
        self.pending(cancelled)?;
        let token = token();
        let sender = self
            .connection
            .unique_name()
            .to_string()
            .trim_start_matches(':')
            .replace('.', "_");
        let expected = format!("{ROOT}/request/{sender}/{token}");
        let options = [("handle_token".into(), value(token))].into();
        let deadline = Instant::now() + CONSENT;
        let handle = match call(self.connection.with_proxy(&self.owner, ROOT, CALL), options) {
            Ok((handle,)) => handle,
            Err(problem) => {
                self.close_request(&expected);
                return Err(error(format!("{method}: {problem}")));
            }
        };
        if *handle != expected {
            self.close_request(&expected);
            return Err(error("the portal returned a foreign request handle"));
        }
        let result = (|| {
            loop {
                self.pending(cancelled)?;
                if Instant::now() >= deadline {
                    return Err(error("timed out waiting for shortcut consent"));
                }
                for (path, response) in self.responses.try_iter() {
                    if path == handle.as_ref() {
                        let (code, results) = response?;
                        return match code {
                            0 => Ok(results),
                            1 => Err(error("shortcut consent was cancelled")),
                            _ => Err(error("shortcut consent was denied or failed")),
                        };
                    }
                }
                self.connection.process(POLL).map_err(error)?;
            }
        })();
        if result.is_err() {
            self.close_request(&handle);
        }
        result
    }

    fn pending(&self, cancelled: &AtomicBool) -> Result<(), String> {
        if cancelled.load(Ordering::Acquire) {
            return Err(error("shortcut request cancelled"));
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(error(
                "the desktop shortcut session closed; retry to reconnect",
            ));
        }
        Ok(())
    }

    fn close_request(&self, path: &str) {
        let _: Result<(), _> = self
            .connection
            .with_proxy(&self.owner, path, CALL)
            .method_call(REQUEST, "Close", ());
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _: Result<(), _> = self
            .connection
            .with_proxy(&self.owner, &self.path, CALL)
            .method_call(SESSION, "Close", ());
    }
}

fn ours(message: &dbus::Message, owner: &str, path: &str) -> bool {
    message.sender().is_some_and(|sender| &*sender == owner)
        && message.path().is_some_and(|actual| &*actual == path)
}

fn revoke(dispatcher: &Dispatcher) {
    let mut routes = dispatcher.routes.lock().unwrap();
    routes.bindings.clear();
    routes.clear();
}

fn publish(
    bindings: Bindings,
    triggers: BTreeMap<String, String>,
    configurable: bool,
    dispatcher: &Dispatcher,
    status: &Mutex<PortalShortcutStatus>,
) {
    let mut routes = dispatcher.routes.lock().unwrap();
    routes.clear();
    routes.bindings = bindings;
    drop(routes);
    *status.lock().unwrap() = PortalShortcutStatus::Bound {
        configurable,
        triggers,
        configuration_error: None,
    };
    (dispatcher.wake)();
}

fn bound_shortcuts(
    results: &PropMap,
    desired: &Bindings,
) -> Result<(Bindings, BTreeMap<String, String>), String> {
    let shortcuts = results
        .get("shortcuts")
        .and_then(|v| v.0.as_iter())
        .ok_or_else(|| error("missing bound shortcuts"))?;
    let mut bindings = Bindings::new();
    let mut triggers = BTreeMap::new();
    for shortcut in shortcuts {
        let mut fields = shortcut
            .as_iter()
            .ok_or_else(|| error("invalid shortcut tuple"))?;
        let id = fields
            .next()
            .and_then(RefArg::as_str)
            .ok_or_else(|| error("invalid shortcut ID"))?;
        let binding: Binding = *desired
            .values()
            .find(|b| b.action.id() == id)
            .ok_or_else(|| error("the portal bound a foreign shortcut ID"))?;
        let mut properties = fields
            .next()
            .and_then(RefArg::as_iter)
            .ok_or_else(|| error("invalid shortcut properties"))?;
        if fields.next().is_some() || triggers.contains_key(id) {
            return Err(error("duplicate or invalid shortcut tuple"));
        }
        let mut trigger = "Assigned by desktop".to_owned();
        while let Some(name) = properties.next() {
            let item = properties
                .next()
                .ok_or_else(|| error("invalid shortcut property"))?;
            if name.as_str() == Some("trigger_description") {
                trigger = item
                    .as_iter()
                    .and_then(|mut v| v.next())
                    .and_then(RefArg::as_str)
                    .ok_or_else(|| error("invalid trigger description"))?
                    .to_owned();
            }
        }
        bindings.insert(binding.key.id(), binding);
        triggers.insert(id.to_owned(), trigger);
    }
    Ok((bindings, triggers))
}

fn preferred_trigger(key: HotKey) -> Option<String> {
    // XDG uses XKB base-layer keysyms, not physical Code names. Unknown keys
    // simply omit the preference; the desktop still owns the actual binding.
    let name = key.key.to_string();
    let symbol = if let Some(letter) = name
        .strip_prefix("Key")
        .filter(|v| v.len() == 1 && v.bytes().all(|b| b.is_ascii_alphabetic()))
    {
        letter.to_ascii_lowercase()
    } else if let Some(digit) = name
        .strip_prefix("Digit")
        .filter(|v| v.len() == 1 && v.bytes().all(|b| b.is_ascii_digit()))
    {
        digit.to_owned()
    } else if name
        .strip_prefix('F')
        .and_then(|v| v.parse::<u8>().ok())
        .is_some_and(|n| (1..=35).contains(&n))
    {
        name
    } else {
        match name.as_str() {
            "Enter" => "Return",
            "Tab" => "Tab",
            "Space" => "space",
            "Backspace" => "BackSpace",
            "PrintScreen" => "Print",
            "Pause" => "Pause",
            "ArrowUp" => "Up",
            "ArrowDown" => "Down",
            "ArrowLeft" => "Left",
            "ArrowRight" => "Right",
            "Home" => "Home",
            "End" => "End",
            "PageUp" => "Page_Up",
            "PageDown" => "Page_Down",
            "Insert" => "Insert",
            "Delete" => "Delete",
            "Minus" => "minus",
            "Equal" => "equal",
            "Backquote" => "grave",
            "Backslash" => "backslash",
            "BracketLeft" => "bracketleft",
            "BracketRight" => "bracketright",
            "Comma" => "comma",
            "Period" => "period",
            "Semicolon" => "semicolon",
            "Slash" => "slash",
            "Quote" => "apostrophe",
            _ => return None,
        }
        .to_owned()
    };
    let supported = Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT | Modifiers::SUPER;
    if !(key.mods - supported).is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for (modifier, name) in [
        (Modifiers::CONTROL, "CTRL"),
        (Modifiers::ALT, "ALT"),
        (Modifiers::SHIFT, "SHIFT"),
        (Modifiers::SUPER, "LOGO"),
    ] {
        if key.mods.contains(modifier) {
            parts.push(name.to_owned());
        }
    }
    parts.push(symbol);
    Some(parts.join("+"))
}

fn description(action: CaptureShortcut) -> &'static str {
    match action {
        CaptureShortcut::NewCapture => "Open capture menu or restore recording controls",
        CaptureShortcut::Region => "Take a region screenshot",
        CaptureShortcut::Window => "Take a window screenshot",
        CaptureShortcut::Display => "Take a screenshot",
        CaptureShortcut::RecordRegion => "Record a region",
        CaptureShortcut::RecordWindow => "Record a window",
        CaptureShortcut::RecordDisplay => "Record a display",
    }
}
fn value(item: impl RefArg + 'static) -> Variant<Box<dyn RefArg>> {
    Variant(Box::new(item))
}
fn token() -> String {
    format!("captures_{}", uuid::Uuid::new_v4().simple())
}
fn error(problem: impl std::fmt::Display) -> String {
    format!("Desktop global shortcuts: {problem}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use global_hotkey::hotkey::Code;

    #[test]
    fn preferred_triggers_use_xdg_modifiers_and_base_keysyms() {
        for (chord, expected) in [
            ("Super+Ctrl+Shift+S", "CTRL+SHIFT+LOGO+s"),
            ("Alt+Enter", "ALT+Return"),
            ("Ctrl+Shift+4", "CTRL+SHIFT+4"),
            ("Ctrl+PageDown", "CTRL+Page_Down"),
            ("F7", "F7"),
        ] {
            assert_eq!(
                preferred_trigger(chord.parse().unwrap()).as_deref(),
                Some(expected)
            );
        }
        assert_eq!(preferred_trigger(HotKey::new(None, Code::Numpad1)), None);
    }

    #[test]
    fn returned_subset_and_desktop_triggers_replace_preferences_without_inventing_grants() {
        let first = Binding {
            key: HotKey::new(None, Code::F7),
            action: CaptureShortcut::Display,
        };
        let second = Binding {
            key: HotKey::new(None, Code::F9),
            action: CaptureShortcut::RecordWindow,
        };
        let desired = [(first.key.id(), first), (second.key.id(), second)].into();
        let shortcut = |id: &str| {
            (
                id.to_owned(),
                [("trigger_description".into(), value("Alt+K".to_owned()))].into(),
            )
        };
        let results =
            |shortcuts: Vec<(String, PropMap)>| [("shortcuts".into(), value(shortcuts))].into();
        let (actual, triggers) =
            bound_shortcuts(&results(vec![shortcut("record_window")]), &desired).unwrap();
        assert_eq!(actual, [(second.key.id(), second)].into());
        assert_eq!(triggers, [("record_window".into(), "Alt+K".into())].into());
        assert!(
            bound_shortcuts(&results(vec![]), &desired)
                .unwrap()
                .0
                .is_empty()
        );
        assert!(
            bound_shortcuts(
                &results(vec![shortcut("display"), shortcut("display")]),
                &desired
            )
            .is_err()
        );
        assert!(bound_shortcuts(&results(vec![shortcut("foreign")]), &desired).is_err());
        assert!(bound_shortcuts(&PropMap::new(), &desired).is_err());
    }
}
