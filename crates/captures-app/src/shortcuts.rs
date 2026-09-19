//! Event-loop-owned screenshot shortcuts. The process-wide dispatcher also
//! serves temporary capture Escape; hosts wake on events, never poll a timer.
use captures_settings::AppSettings;
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
    rc::Rc,
    sync::{Arc, Mutex, Once},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureShortcut {
    Region,
    Window,
    Display,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Binding {
    key: HotKey,
    action: CaptureShortcut,
}
type Bindings = BTreeMap<u32, Binding>;

fn bindings(settings: &AppSettings) -> Result<Bindings, String> {
    let mut result = Bindings::new();
    for (text, action) in [
        (&settings.region_shortcut, CaptureShortcut::Region),
        (&settings.window_shortcut, CaptureShortcut::Window),
        (&settings.display_shortcut, CaptureShortcut::Display),
    ] {
        let key = text
            .parse::<HotKey>()
            .map_err(|error| format!("Invalid {action:?} shortcut: {error}"))?;
        if key.id() == HotKey::new(None, Code::Escape).id() {
            return Err("Escape is reserved for capture cancellation".into());
        }
        if result.insert(key.id(), Binding { key, action }).is_some() {
            return Err("Screenshot shortcuts must use different keys".into());
        }
    }
    Ok(result)
}

#[derive(Default)]
struct Routes {
    bindings: Bindings,
    armed: BTreeSet<u32>,
    pending: Option<CaptureShortcut>,
    enabled: bool,
}

impl Routes {
    fn clear(&mut self) {
        self.armed.clear();
        self.pending = None;
    }

    fn event(&mut self, id: u32, state: HotKeyState, blocked: bool) -> bool {
        if !self.enabled || blocked || id == HotKey::new(None, Code::Escape).id() {
            self.clear();
            return false;
        }
        let Some(binding) = self.bindings.get(&id) else {
            return false;
        };
        match state {
            HotKeyState::Pressed => {
                self.armed.insert(id);
                false
            }
            HotKeyState::Released => {
                if self.armed.remove(&id) && self.pending.is_none() {
                    self.pending = Some(binding.action);
                    true
                } else {
                    false
                }
            }
        }
    }
}

struct Dispatcher {
    routes: Mutex<Routes>,
    wake: Box<dyn Fn() + Send + Sync>,
}
static DISPATCHER: Mutex<Option<Arc<Dispatcher>>> = Mutex::new(None);
static INSTALL: Once = Once::new();

pub(crate) fn install_dispatcher() {
    // global-hotkey 0.8 uses a OnceCell: later set_event_handler calls cannot
    // replace the first handler. Every native hotkey owner uses this one sink.
    INSTALL.call_once(|| GlobalHotKeyEvent::set_event_handler(Some(dispatch)));
}

fn dispatch(event: GlobalHotKeyEvent) {
    if event.id == HotKey::new(None, Code::Escape).id() && event.state == HotKeyState::Pressed {
        crate::capture_flow::escape();
    }
    let dispatcher = DISPATCHER.lock().unwrap().clone();
    if let Some(dispatcher) = dispatcher {
        let wake = dispatcher.routes.lock().unwrap().event(
            event.id,
            event.state,
            crate::capture_flow::active(),
        );
        if wake {
            (dispatcher.wake)();
        }
    }
}

trait Registration {
    fn register(&self, key: HotKey) -> Result<(), String>;
    fn unregister(&self, key: HotKey) -> Result<(), String>;
}
impl Registration for GlobalHotKeyManager {
    fn register(&self, key: HotKey) -> Result<(), String> {
        GlobalHotKeyManager::register(self, key).map_err(|error| error.to_string())
    }
    fn unregister(&self, key: HotKey) -> Result<(), String> {
        GlobalHotKeyManager::unregister(self, key).map_err(|error| error.to_string())
    }
}

fn rebind(manager: &impl Registration, old: &Bindings, new: &Bindings) -> Result<(), String> {
    let mut added = Vec::new();
    for (id, binding) in new {
        if old.contains_key(id) {
            continue;
        }
        if let Err(error) = manager.register(binding.key) {
            // X11 registration may have grabbed some modifier variants before
            // failing. Unregister attempts every variant, even absent map state.
            let _ = manager.unregister(binding.key);
            let mut rollback_failed = false;
            for key in added.into_iter().rev() {
                rollback_failed |= manager.unregister(key).is_err();
            }
            return Err(format!(
                "Could not register {:?}: {error}{}",
                binding.action,
                if rollback_failed {
                    "; shortcut cleanup failed; restart the native app"
                } else {
                    ""
                }
            ));
        }
        added.push(binding.key);
    }
    let mut removed = Vec::new();
    for (id, binding) in old {
        if new.contains_key(id) {
            continue;
        }
        if let Err(error) = manager.unregister(binding.key) {
            let mut rollback_failed = false;
            for key in removed {
                rollback_failed |= manager.register(key).is_err();
            }
            for key in added {
                rollback_failed |= manager.unregister(key).is_err();
            }
            return Err(format!(
                "Could not release {:?}: {error}{}",
                binding.action,
                if rollback_failed {
                    "; shortcut rollback failed; restart the native app"
                } else {
                    ""
                }
            ));
        }
        removed.push(binding.key);
    }
    Ok(())
}

/// One live host owns this object on the AppKit/winit event-loop thread. OS
/// callbacks only queue an action and call `wake`; invoke `next_action` on the
/// host thread. Never configure fixture scenes or change OS screenshot settings.
pub struct CaptureShortcuts {
    manager: GlobalHotKeyManager,
    dispatcher: Arc<Dispatcher>,
    _event_loop_thread: PhantomData<Rc<()>>,
}

impl CaptureShortcuts {
    pub fn new(
        settings: &AppSettings,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        // Validate before touching OS state or claiming the singleton route.
        bindings(settings)?;
        install_dispatcher();
        let manager = GlobalHotKeyManager::new().map_err(|error| error.to_string())?;
        let dispatcher = Arc::new(Dispatcher {
            routes: Mutex::new(Routes::default()),
            wake: Box::new(wake),
        });
        {
            let mut owner = DISPATCHER.lock().unwrap();
            if owner.is_some() {
                return Err("Capture shortcuts already have a live owner".into());
            }
            *owner = Some(dispatcher.clone());
        }
        let mut shortcuts = Self {
            manager,
            dispatcher,
            _event_loop_thread: PhantomData,
        };
        shortcuts.update(settings)?;
        shortcuts.set_enabled(true);
        Ok(shortcuts)
    }

    /// Add new chords before removing old ones. Parse/conflict failure retains
    /// the old mapping; OS rollback failure is reported, never silently ignored.
    pub fn update(&mut self, settings: &AppSettings) -> Result<(), String> {
        let next = bindings(settings)?;
        let (old, enabled) = {
            let mut routes = self.dispatcher.routes.lock().unwrap();
            if routes.bindings == next {
                return Ok(());
            }
            let enabled = routes.enabled;
            routes.enabled = false;
            routes.clear();
            (routes.bindings.clone(), enabled)
        };
        let result = rebind(&self.manager, &old, &next);
        let mut routes = self.dispatcher.routes.lock().unwrap();
        if result.is_ok() {
            routes.bindings = next;
        }
        routes.enabled = enabled;
        result
    }

    /// Disable while editing shortcuts or while the host is preparing capture.
    /// An old release/wake cannot launch after suppression or reconfiguration.
    pub fn set_enabled(&self, enabled: bool) {
        let mut routes = self.dispatcher.routes.lock().unwrap();
        routes.enabled = enabled;
        if !enabled {
            routes.clear();
        }
    }

    pub fn next_action(&self) -> Option<CaptureShortcut> {
        let mut routes = self.dispatcher.routes.lock().unwrap();
        let pending = routes.pending.take();
        (routes.enabled && !crate::capture_flow::active())
            .then_some(pending)
            .flatten()
    }
}

impl Drop for CaptureShortcuts {
    fn drop(&mut self) {
        self.set_enabled(false);
        *DISPATCHER.lock().unwrap() = None;
        // Explicitly unregister before manager teardown; X11 manager Drop only
        // queues connection shutdown rather than waiting for its worker to exit.
        // Do not hold a callback's mutex while waiting for the X11 worker.
        let bindings = self.dispatcher.routes.lock().unwrap().bindings.clone();
        for binding in bindings.values() {
            let _ = self.manager.unregister(binding.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn settings() -> AppSettings {
        AppSettings {
            region_shortcut: "Ctrl+Shift+1".into(),
            window_shortcut: "Ctrl+Shift+2".into(),
            display_shortcut: "Ctrl+Shift+3".into(),
            ..AppSettings::default()
        }
    }

    #[test]
    fn aliases_conflict_and_escape_is_reserved_before_registration() {
        let mut settings = settings();
        settings.window_shortcut = "Control+Shift+1".into();
        assert!(bindings(&settings).unwrap_err().contains("different"));
        settings.window_shortcut = "Escape".into();
        assert!(bindings(&settings).unwrap_err().contains("reserved"));
        settings.window_shortcut = "not a hotkey".into();
        assert!(bindings(&settings).is_err());
    }

    #[test]
    fn only_matched_releases_launch_and_suppression_invalidates_held_or_queued_keys() {
        let bindings = bindings(&settings()).unwrap();
        let region = bindings
            .iter()
            .find(|(_, binding)| binding.action == CaptureShortcut::Region)
            .unwrap()
            .0
            .to_owned();
        let mut routes = Routes {
            bindings,
            enabled: true,
            ..Routes::default()
        };
        assert!(!routes.event(region, HotKeyState::Released, false));
        assert!(!routes.event(region, HotKeyState::Pressed, false));
        assert!(!routes.event(region, HotKeyState::Pressed, false));
        assert!(routes.event(region, HotKeyState::Released, false));
        assert_eq!(routes.pending.take(), Some(CaptureShortcut::Region));
        assert!(!routes.event(region, HotKeyState::Released, false));
        routes.event(region, HotKeyState::Pressed, false);
        assert!(!routes.event(region, HotKeyState::Released, true));
        assert!(routes.pending.is_none());
        routes.event(region, HotKeyState::Pressed, false);
        routes.event(
            HotKey::new(None, Code::Escape).id(),
            HotKeyState::Pressed,
            false,
        );
        assert!(!routes.event(region, HotKeyState::Released, false));
        routes.event(region, HotKeyState::Pressed, false);
        routes.event(region, HotKeyState::Released, false);
        routes.enabled = false;
        routes.event(region, HotKeyState::Pressed, false);
        routes.enabled = true;
        assert!(routes.pending.is_none());
        assert!(!routes.event(region, HotKeyState::Released, false));
    }

    #[derive(Default)]
    struct Backend {
        keys: RefCell<BTreeSet<u32>>,
        fail: Option<u32>,
        fail_cleanup: Option<u32>,
    }
    impl Registration for Backend {
        fn register(&self, key: HotKey) -> Result<(), String> {
            // Deliberately model a partial OS grab before the failure.
            self.keys.borrow_mut().insert(key.id());
            if self.fail == Some(key.id()) {
                Err("occupied".into())
            } else {
                Ok(())
            }
        }
        fn unregister(&self, key: HotKey) -> Result<(), String> {
            self.keys.borrow_mut().remove(&key.id());
            if self.fail_cleanup == Some(key.id()) {
                Err("cleanup error".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn registration_failure_cleans_partial_additions_and_retains_old_chords() {
        let old = bindings(&settings()).unwrap();
        let mut changed = settings();
        changed.region_shortcut = "Alt+4".into();
        changed.window_shortcut = "Alt+5".into();
        let next = bindings(&changed).unwrap();
        let failed = next
            .keys()
            .filter(|id| !old.contains_key(id))
            .max()
            .copied()
            .unwrap();
        let backend = Backend {
            keys: RefCell::new(old.keys().copied().collect()),
            fail: Some(failed),
            ..Backend::default()
        };
        assert!(rebind(&backend, &old, &next).is_err());
        assert_eq!(*backend.keys.borrow(), old.keys().copied().collect());
    }

    #[test]
    fn swapping_actions_reuses_chords_without_reregistering() {
        let old = bindings(&settings()).unwrap();
        let mut changed = settings();
        std::mem::swap(&mut changed.region_shortcut, &mut changed.window_shortcut);
        let next = bindings(&changed).unwrap();
        let backend = Backend {
            keys: RefCell::new(old.keys().copied().collect()),
            fail: old.keys().next().copied(),
            ..Backend::default()
        };
        rebind(&backend, &old, &next).unwrap();
        assert_eq!(*backend.keys.borrow(), old.keys().copied().collect());
        let key = "Ctrl+Shift+1".parse::<HotKey>().unwrap().id();
        assert_eq!(next[&key].action, CaptureShortcut::Window);
    }

    #[test]
    fn rollback_attempts_every_added_key_even_after_a_cleanup_error() {
        let old = bindings(&settings()).unwrap();
        let mut changed = settings();
        changed.region_shortcut = "Alt+4".into();
        changed.window_shortcut = "Alt+5".into();
        changed.display_shortcut = "Alt+6".into();
        let next = bindings(&changed).unwrap();
        let keys: Vec<_> = next.keys().copied().collect();
        let backend = Backend {
            keys: RefCell::new(old.keys().copied().collect()),
            fail: Some(keys[2]),
            fail_cleanup: Some(keys[1]),
        };
        assert!(
            rebind(&backend, &old, &next)
                .unwrap_err()
                .contains("cleanup failed")
        );
        assert_eq!(*backend.keys.borrow(), old.keys().copied().collect());
    }
}
