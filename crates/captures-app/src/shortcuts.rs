//! Event-loop-owned capture shortcuts. The process-wide dispatcher also
//! serves temporary capture Escape; hosts wake on events, never poll a timer.
mod recording;
pub use recording::{
    ShortcutKeyEvent, ShortcutPlatform, ShortcutRecording, record_shortcut, shortcut_display_tokens,
};

use captures_settings::AppSettings;
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
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
    NewCapture,
    Region,
    Window,
    Display,
    RecordRegion,
    RecordWindow,
    RecordDisplay,
}

impl CaptureShortcut {
    pub fn is_recording(self) -> bool {
        matches!(
            self,
            Self::RecordRegion | Self::RecordWindow | Self::RecordDisplay
        )
    }
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
        (&settings.new_capture_shortcut, CaptureShortcut::NewCapture),
        (&settings.region_shortcut, CaptureShortcut::Region),
        (&settings.window_shortcut, CaptureShortcut::Window),
        (&settings.display_shortcut, CaptureShortcut::Display),
        (
            &settings.recording.video_shortcut,
            CaptureShortcut::RecordRegion,
        ),
        (
            &settings.recording.window_shortcut,
            CaptureShortcut::RecordWindow,
        ),
        (
            &settings.recording.display_shortcut,
            CaptureShortcut::RecordDisplay,
        ),
    ] {
        let key = text
            .parse::<HotKey>()
            .map_err(|error| format!("Invalid {action:?} shortcut: {error}"))?;
        if key.id() == HotKey::new(None, Code::Escape).id() {
            return Err("Escape is reserved for capture cancellation".into());
        }
        if result.insert(key.id(), Binding { key, action }).is_some() {
            return Err("Capture shortcuts must use different keys".into());
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
    restore_only: bool,
    /// A running or paused recording owns the flow and accepts a display
    /// screenshot (`capture_error::display_route`).
    recording_screenshot: bool,
    suspended: bool,
    restoring: bool,
    selector_generation: Option<u64>,
}

impl Routes {
    fn clear(&mut self) {
        self.armed.clear();
        self.pending = None;
    }

    fn set_selector_generation(&mut self, generation: Option<u64>) {
        if self.selector_generation != generation {
            self.clear();
            self.selector_generation = generation;
        }
    }

    /// Shipping lets the display shortcut through while a recording runs,
    /// even with its controls hidden; every other busy binding stays blocked.
    fn routes_recording_display(&self, action: Option<CaptureShortcut>) -> bool {
        self.recording_screenshot
            && self.selector_generation.is_none()
            && action == Some(CaptureShortcut::Display)
    }

    fn event(&mut self, id: u32, state: HotKeyState, blocked: bool) -> bool {
        let recording_display =
            self.routes_recording_display(self.bindings.get(&id).map(|binding| binding.action));
        if !self.enabled
            || (self.suspended && !self.restoring)
            || (blocked && !recording_display)
            || id == HotKey::new(None, Code::Escape).id()
        {
            self.clear();
            return false;
        }
        let Some(binding) = self.bindings.get(&id) else {
            return false;
        };
        if self.restore_only && binding.action != CaptureShortcut::NewCapture && !recording_display
        {
            self.armed.remove(&id);
            return false;
        }
        if self.selector_generation.is_some() && binding.action == CaptureShortcut::NewCapture {
            return false;
        }
        match state {
            HotKeyState::Pressed => {
                self.armed.insert(id);
                false
            }
            HotKeyState::Released => {
                if self.armed.remove(&id) && self.pending.is_none() {
                    self.pending = Some(binding.action);
                    !self.suspended
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

/// Win/Super+Shift+S. Explorer/Snipping Tool own it before `RegisterHotKey`
/// on Windows, so shipping (`skip_windows_os_owned_super_shift_s`) never
/// registers it there and a low-level hook routes the chord instead.
fn super_shift_s() -> HotKey {
    HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyS)
}

fn os_owned(key: &HotKey, windows: bool) -> bool {
    windows && key.id() == super_shift_s().id()
}

fn on_win_shift_s(phase: captures_session::WinShiftSPhase) {
    let state = match phase {
        captures_session::WinShiftSPhase::Pressed => HotKeyState::Pressed,
        captures_session::WinShiftSPhase::Released => HotKeyState::Released,
    };
    dispatch(GlobalHotKeyEvent {
        id: super_shift_s().id(),
        state,
    });
}

/// Enable the Win+Shift+S interceptor exactly while that chord is a live
/// binding (never while Preferences has suspended registration). No-op off
/// Windows: the `captures_session` hook is a stub there.
fn sync_win_shift_s_takeover(registered: &Bindings) {
    let takeover = cfg!(target_os = "windows") && registered.contains_key(&super_shift_s().id());
    captures_session::set_win_shift_s_takeover_enabled(takeover);
    if !takeover {
        captures_session::set_win_shift_s_handler(None);
        return;
    }
    captures_session::set_win_shift_s_handler(Some(on_win_shift_s));
    if let Err(error) = captures_session::ensure_win_shift_s_takeover() {
        eprintln!("could not take over Win+Shift+S from Snipping Tool: {error}");
    }
}

fn dispatch(event: GlobalHotKeyEvent) {
    if event.id == HotKey::new(None, Code::Escape).id() && event.state == HotKeyState::Pressed {
        crate::capture_flow::escape();
    }
    let dispatcher = DISPATCHER.lock().unwrap().clone();
    if let Some(dispatcher) = dispatcher {
        let wake = {
            let mut routes = dispatcher.routes.lock().unwrap();
            let blocked = !routes.restore_only
                && !crate::capture_flow::shortcuts_allowed(routes.selector_generation);
            routes.event(event.id, event.state, blocked)
        };
        if wake {
            (dispatcher.wake)();
        }
    }
}

trait Registration {
    fn register(&self, key: HotKey) -> Result<(), String>;
    fn unregister(&self, key: HotKey) -> Result<(), String>;
    fn register_all(&self, keys: &[HotKey]) -> Result<(), String> {
        let mut result = Ok(());
        for key in keys {
            result = result.and(self.register(*key));
        }
        result
    }
    fn unregister_all(&self, keys: &[HotKey]) -> Result<(), String> {
        let mut result = Ok(());
        for key in keys {
            result = result.and(self.unregister(*key));
        }
        result
    }
}
/// OS-owned chords are routed by a hook, never registered with the manager.
fn os_registrable(keys: &[HotKey]) -> Vec<HotKey> {
    let windows = cfg!(target_os = "windows");
    keys.iter()
        .copied()
        .filter(|key| !os_owned(key, windows))
        .collect()
}

impl Registration for GlobalHotKeyManager {
    fn register(&self, key: HotKey) -> Result<(), String> {
        Registration::register_all(self, &[key])
    }
    fn unregister(&self, key: HotKey) -> Result<(), String> {
        Registration::unregister_all(self, &[key])
    }
    fn register_all(&self, keys: &[HotKey]) -> Result<(), String> {
        let keys = os_registrable(keys);
        if keys.is_empty() {
            return Ok(());
        }
        GlobalHotKeyManager::register_all(self, &keys).map_err(|error| error.to_string())
    }
    fn unregister_all(&self, keys: &[HotKey]) -> Result<(), String> {
        let keys = os_registrable(keys);
        if keys.is_empty() {
            return Ok(());
        }
        GlobalHotKeyManager::unregister_all(self, &keys).map_err(|error| error.to_string())
    }
}

fn rebind(manager: &impl Registration, old: &Bindings, new: &Bindings) -> Result<(), String> {
    let added: Vec<_> = new
        .iter()
        .filter(|(id, _)| !old.contains_key(id))
        .map(|(_, binding)| binding.key)
        .collect();
    let removed: Vec<_> = old
        .iter()
        .filter(|(id, _)| !new.contains_key(id))
        .map(|(_, binding)| binding.key)
        .collect();
    // X11 processes one registration command per 50ms worker tick. Batching
    // avoids seven serial ticks on focus changes, but is NOT transactional:
    // later keys can be grabbed even after an earlier key fails.
    if !added.is_empty()
        && let Err(error) = manager.register_all(&added)
    {
        let mut rollback_failed = false;
        for key in added {
            rollback_failed |= manager.unregister(key).is_err();
        }
        return Err(format!(
            "Could not register capture shortcuts: {error}{}",
            if rollback_failed {
                "; shortcut cleanup failed; restart the native app"
            } else {
                ""
            }
        ));
    }
    if !removed.is_empty()
        && let Err(error) = manager.unregister_all(&removed)
    {
        let mut rollback_failed = false;
        // On failure attempt every rollback, including the partly removed key.
        for key in removed {
            rollback_failed |= manager.register(key).is_err();
        }
        for key in added {
            rollback_failed |= manager.unregister(key).is_err();
        }
        return Err(format!(
            "Could not release capture shortcuts: {error}{}",
            if rollback_failed {
                "; shortcut rollback failed; restart the native app"
            } else {
                ""
            }
        ));
    }
    Ok(())
}

fn sync_bindings(
    manager: &impl Registration,
    registered: &mut Bindings,
    desired: &Bindings,
    suspended: bool,
) -> Result<(), String> {
    let empty = Bindings::new();
    let next = if suspended { &empty } else { desired };
    rebind(manager, registered, next)?;
    registered.clone_from(next);
    Ok(())
}

fn suspend_routes(
    manager: &impl Registration,
    registered: &mut Bindings,
    routes: &Mutex<Routes>,
    suspended: bool,
) -> Result<(), String> {
    let desired = {
        let mut routes = routes.lock().unwrap();
        let settled = if suspended {
            registered.is_empty()
        } else {
            *registered == routes.bindings
        };
        if routes.suspended == suspended && settled {
            return Ok(());
        }
        routes.suspended = true;
        // Grabs become live one at a time. Retain presses received while they
        // are restored, but never wake/deliver until the full restore succeeds.
        routes.restoring = !suspended;
        routes.clear();
        routes.bindings.clone()
    };
    // Never hold a callback's mutex while waiting on an OS hotkey worker.
    let result = sync_bindings(manager, registered, &desired, suspended);
    let mut routes = routes.lock().unwrap();
    routes.restoring = false;
    if result.is_ok() {
        routes.suspended = suspended;
    } else {
        routes.clear();
    }
    result
}

/// One live host owns this object on the AppKit/winit event-loop thread. OS
/// callbacks only queue an action and call `wake`; invoke `next_action` on the
/// host thread. Never construct it for fixture scenes: like shipping, it
/// unbinds overlapping OS screenshot keys ([`crate::system_shortcuts`]) at
/// startup and whenever the bindings change.
pub struct CaptureShortcuts {
    manager: GlobalHotKeyManager,
    dispatcher: Arc<Dispatcher>,
    registered: Bindings,
    takeover: crate::system_shortcuts::Takeover,
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
            registered: Bindings::new(),
            takeover: crate::system_shortcuts::Takeover::default(),
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
        let (enabled, suspended) = {
            let mut routes = self.dispatcher.routes.lock().unwrap();
            if routes.bindings == next {
                return Ok(());
            }
            let enabled = routes.enabled;
            routes.enabled = false;
            routes.clear();
            (enabled, routes.suspended)
        };
        // Shipping frees overlapping system keys before claiming the chords.
        self.takeover = crate::system_shortcuts::take_over_current_os(settings);
        let result = sync_bindings(&self.manager, &mut self.registered, &next, suspended);
        sync_win_shift_s_takeover(&self.registered);
        let mut routes = self.dispatcher.routes.lock().unwrap();
        if result.is_ok() {
            routes.bindings = next;
        }
        routes.enabled = enabled;
        result
    }

    /// Release OS grabs while Preferences owns keyboard focus so its recorder
    /// can receive existing chords. Keep desired bindings across edits, then
    /// restore them on blur. Failure leaves routing suspended and is retryable.
    pub fn set_suspended(&mut self, suspended: bool) -> Result<(), String> {
        let result = suspend_routes(
            &self.manager,
            &mut self.registered,
            &self.dispatcher.routes,
            suspended,
        );
        sync_win_shift_s_takeover(&self.registered);
        result?;
        let pending = {
            let routes = self.dispatcher.routes.lock().unwrap();
            !routes.suspended && routes.pending.is_some()
        };
        if pending {
            (self.dispatcher.wake)();
        }
        Ok(())
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

    /// While recording controls are hidden, retain only the configured New
    /// Capture chord as a restoration route. Other capture bindings remain
    /// suppressed exactly as they are for every other busy recording phase.
    pub fn set_restore_only(&self, restore_only: bool) {
        let mut routes = self.dispatcher.routes.lock().unwrap();
        if routes.restore_only != restore_only {
            routes.clear();
            routes.restore_only = restore_only;
        }
    }

    /// While a running or paused recording owns the flow, route the display
    /// shortcut to a screenshot beside it (`capture_error::display_route`).
    pub fn set_recording_screenshot(&self, allowed: bool) {
        let mut routes = self.dispatcher.routes.lock().unwrap();
        if routes.recording_screenshot != allowed {
            routes.clear();
            routes.recording_screenshot = allowed;
        }
    }

    /// Route target shortcuts to an already-open New Capture selector, never
    /// to a new capture. Set only during selection, and clear before countdown,
    /// display preparation or cancellation. Scope changes discard held/queued
    /// chords. A stale generation cannot route after cancellation or commit.
    /// Callback enablement and Preferences registration suspension still apply.
    pub fn set_selector_generation(&self, generation: Option<u64>) {
        self.dispatcher
            .routes
            .lock()
            .unwrap()
            .set_selector_generation(generation);
    }

    /// The latest system-key takeover, once. AppKit also disables the
    /// returned macOS symbolic hotkeys live; errors are informational.
    pub fn take_system_takeover(&mut self) -> crate::system_shortcuts::Takeover {
        std::mem::take(&mut self.takeover)
    }

    pub fn next_action(&self) -> Option<CaptureShortcut> {
        let mut routes = self.dispatcher.routes.lock().unwrap();
        let pending = routes.pending.take();
        let recording_display = routes.routes_recording_display(pending);
        (routes.enabled
            && !routes.suspended
            && (!routes.restore_only
                || pending == Some(CaptureShortcut::NewCapture)
                || recording_display)
            && (routes.restore_only
                || recording_display
                || crate::capture_flow::shortcuts_allowed(routes.selector_generation)))
        .then_some(pending)
        .flatten()
    }
}

impl Drop for CaptureShortcuts {
    fn drop(&mut self) {
        self.set_enabled(false);
        *DISPATCHER.lock().unwrap() = None;
        // Hand Win+Shift+S back to Snipping Tool once nothing routes it.
        sync_win_shift_s_takeover(&Bindings::new());
        // Explicitly unregister before manager teardown; X11 manager Drop only
        // queues connection shutdown rather than waiting for its worker to exit.
        // Do not hold a callback's mutex while waiting for the X11 worker.
        for binding in self.registered.values() {
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
            new_capture_shortcut: "Ctrl+Alt+F10".into(),
            region_shortcut: "Ctrl+Shift+1".into(),
            window_shortcut: "Ctrl+Shift+2".into(),
            display_shortcut: "Ctrl+Shift+3".into(),
            ..AppSettings::default()
        }
    }

    #[test]
    fn windows_routes_win_shift_s_through_the_hook_not_registration() {
        let key = "Super+Shift+S".parse::<HotKey>().unwrap();
        assert_eq!(key.id(), super_shift_s().id());
        assert!(os_owned(&key, true));
        assert!(
            !os_owned(&key, false),
            "other platforms register it normally"
        );
        let other = "Ctrl+Shift+S".parse::<HotKey>().unwrap();
        assert!(!os_owned(&other, true));
        let expected = if cfg!(target_os = "windows") {
            vec![other]
        } else {
            vec![key, other]
        };
        assert_eq!(os_registrable(&[key, other]), expected);
    }

    #[test]
    fn aliases_conflict_and_escape_is_reserved_before_registration() {
        let mut settings = settings();
        settings.new_capture_shortcut = "Control+Shift+1".into();
        assert!(bindings(&settings).unwrap_err().contains("different"));
        settings.new_capture_shortcut = "Escape".into();
        assert!(bindings(&settings).unwrap_err().contains("reserved"));
        settings.new_capture_shortcut = "Control+Alt+F10".into();
        settings.window_shortcut = "Control+Shift+1".into();
        assert!(bindings(&settings).unwrap_err().contains("different"));
        settings.window_shortcut = "Escape".into();
        assert!(bindings(&settings).unwrap_err().contains("reserved"));
        settings.window_shortcut = "not a hotkey".into();
        assert!(bindings(&settings).is_err());
    }

    #[test]
    fn recording_bindings_route_releases_in_idle_and_selector_but_not_busy_or_suspended() {
        let settings = settings();
        let mut routes = Routes {
            bindings: bindings(&settings).unwrap(),
            enabled: true,
            ..Routes::default()
        };
        for (text, action, wire) in [
            (
                &settings.recording.video_shortcut,
                CaptureShortcut::RecordRegion,
                "record_region",
            ),
            (
                &settings.recording.window_shortcut,
                CaptureShortcut::RecordWindow,
                "record_window",
            ),
            (
                &settings.recording.display_shortcut,
                CaptureShortcut::RecordDisplay,
                "record_display",
            ),
        ] {
            assert_eq!(serde_json::to_value(action).unwrap(), wire);
            let key = text.parse::<HotKey>().unwrap().id();
            for scope in [None, Some(17)] {
                routes.set_selector_generation(scope);
                assert!(!routes.event(key, HotKeyState::Released, false));
                assert!(!routes.event(key, HotKeyState::Pressed, false));
                assert!(routes.event(key, HotKeyState::Released, false));
                assert_eq!(routes.pending.take(), Some(action));
                routes.event(key, HotKeyState::Pressed, false);
                assert!(!routes.event(key, HotKeyState::Released, true));
                assert!(routes.pending.is_none());
                routes.suspended = true;
                assert!(!routes.event(key, HotKeyState::Pressed, false));
                routes.suspended = false;
                assert!(!routes.event(key, HotKeyState::Released, false));
                routes.enabled = false;
                assert!(!routes.event(key, HotKeyState::Pressed, false));
                routes.enabled = true;
                assert!(!routes.event(key, HotKeyState::Released, false));
            }
        }
        let mut conflicting = settings.clone();
        conflicting
            .recording
            .window_shortcut
            .clone_from(&settings.region_shortcut);
        assert!(bindings(&conflicting).unwrap_err().contains("different"));
        conflicting.recording.window_shortcut = "Escape".into();
        assert!(bindings(&conflicting).unwrap_err().contains("reserved"));
    }

    #[test]
    fn restore_only_routes_new_capture_and_suppresses_every_other_busy_binding() {
        let settings = settings();
        let mut routes = Routes {
            bindings: bindings(&settings).unwrap(),
            enabled: true,
            restore_only: true,
            ..Routes::default()
        };
        for (text, expected) in [
            (
                &settings.new_capture_shortcut,
                Some(CaptureShortcut::NewCapture),
            ),
            (&settings.region_shortcut, None),
            (&settings.recording.video_shortcut, None),
        ] {
            let id = text.parse::<HotKey>().unwrap().id();
            assert!(!routes.event(id, HotKeyState::Pressed, false));
            assert_eq!(
                routes.event(id, HotKeyState::Released, false),
                expected.is_some()
            );
            assert_eq!(routes.pending.take(), expected);
        }
    }

    #[test]
    fn a_running_recording_routes_only_the_display_shortcut() {
        let settings = settings();
        let id = |text: &str| text.parse::<HotKey>().unwrap().id();
        let display = id(&settings.display_shortcut);
        let mut routes = Routes {
            bindings: bindings(&settings).unwrap(),
            enabled: true,
            recording_screenshot: true,
            ..Routes::default()
        };
        for restore_only in [false, true] {
            routes.restore_only = restore_only;
            // The recording owns the capture flow, so every route is blocked.
            assert!(!routes.event(display, HotKeyState::Pressed, true));
            assert!(routes.event(display, HotKeyState::Released, true));
            assert_eq!(routes.pending.take(), Some(CaptureShortcut::Display));
            for other in [
                &settings.region_shortcut,
                &settings.window_shortcut,
                &settings.recording.video_shortcut,
                &settings.recording.display_shortcut,
            ] {
                assert!(!routes.event(id(other), HotKeyState::Pressed, true));
                assert!(!routes.event(id(other), HotKeyState::Released, true));
                assert!(routes.pending.is_none(), "{other} stays blocked");
            }
        }
        // Countdown, finalizing and a screenshot in progress withdraw the route.
        routes.recording_screenshot = false;
        assert!(!routes.event(display, HotKeyState::Pressed, true));
        assert!(!routes.event(display, HotKeyState::Released, true));
        assert!(routes.pending.is_none());
        routes.recording_screenshot = true;
        routes.suspended = true;
        assert!(!routes.event(display, HotKeyState::Pressed, true));
        routes.suspended = false;
        assert!(!routes.event(display, HotKeyState::Released, true));
        assert!(routes.pending.is_none());
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

    #[test]
    fn selector_routes_targets_but_scope_changes_discard_held_and_pending_chords() {
        let mut routes = Routes {
            bindings: bindings(&settings()).unwrap(),
            enabled: true,
            ..Routes::default()
        };
        let region = "Control+Shift+1".parse::<HotKey>().unwrap().id();
        let window = "Control+Shift+2".parse::<HotKey>().unwrap().id();
        let display = "Control+Shift+3".parse::<HotKey>().unwrap().id();
        let new_capture = "Control+Alt+F10".parse::<HotKey>().unwrap().id();
        routes.event(region, HotKeyState::Pressed, false);
        routes.set_selector_generation(Some(2));
        assert!(!routes.event(region, HotKeyState::Released, false));
        for (key, action) in [
            (region, CaptureShortcut::Region),
            (window, CaptureShortcut::Window),
            (display, CaptureShortcut::Display),
        ] {
            assert!(!routes.event(key, HotKeyState::Pressed, false));
            routes.set_selector_generation(Some(2));
            assert!(routes.event(key, HotKeyState::Released, false));
            assert_eq!(routes.pending.take(), Some(action));
        }
        assert!(!routes.event(new_capture, HotKeyState::Pressed, false));
        assert!(!routes.event(new_capture, HotKeyState::Released, false));
        assert!(routes.pending.is_none());
        routes.event(window, HotKeyState::Pressed, false);
        routes.event(window, HotKeyState::Released, false);
        routes.set_selector_generation(None);
        assert!(routes.pending.is_none());
        routes.set_selector_generation(Some(4));
        routes.event(display, HotKeyState::Pressed, false);
        routes.set_selector_generation(None);
        assert!(!routes.event(display, HotKeyState::Released, false));
        routes.event(new_capture, HotKeyState::Pressed, false);
        assert!(routes.event(new_capture, HotKeyState::Released, false));
        assert_eq!(routes.pending.take(), Some(CaptureShortcut::NewCapture));
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
    fn new_capture_has_distinct_release_routing_and_restores_edited_binding() {
        let old_key = "Control+Alt+F10".parse::<HotKey>().unwrap().id();
        let new_key = "Control+Alt+F11".parse::<HotKey>().unwrap().id();
        let mut registered = bindings(&settings()).unwrap();
        assert_eq!(registered.len(), 7);
        let backend = Backend {
            keys: RefCell::new(registered.keys().copied().collect()),
            ..Backend::default()
        };
        let routes = Mutex::new(Routes {
            bindings: registered.clone(),
            enabled: true,
            ..Routes::default()
        });
        {
            let mut state = routes.lock().unwrap();
            assert!(!state.event(old_key, HotKeyState::Released, false));
            assert!(!state.event(old_key, HotKeyState::Pressed, false));
            assert!(state.event(old_key, HotKeyState::Released, false));
            assert_eq!(state.pending.take(), Some(CaptureShortcut::NewCapture));
        }
        assert_eq!(
            serde_json::to_value(CaptureShortcut::NewCapture).unwrap(),
            "new_capture"
        );

        suspend_routes(&backend, &mut registered, &routes, true).unwrap();
        let mut settings = settings();
        settings.new_capture_shortcut = "Control+Alt+F11".into();
        let desired = bindings(&settings).unwrap();
        sync_bindings(&backend, &mut registered, &desired, true).unwrap();
        routes.lock().unwrap().bindings = desired;
        assert!(backend.keys.borrow().is_empty());
        assert!(
            !routes
                .lock()
                .unwrap()
                .event(new_key, HotKeyState::Pressed, false)
        );
        suspend_routes(&backend, &mut registered, &routes, false).unwrap();
        assert!(!backend.keys.borrow().contains(&old_key));
        assert!(backend.keys.borrow().contains(&new_key));
        let mut state = routes.lock().unwrap();
        assert!(!state.event(new_key, HotKeyState::Released, false));
        assert!(!state.event(new_key, HotKeyState::Pressed, false));
        assert!(state.event(new_key, HotKeyState::Released, false));
        assert_eq!(state.pending.take(), Some(CaptureShortcut::NewCapture));
    }

    #[test]
    fn suspension_releases_grabs_defers_edits_and_restores_only_latest_bindings() {
        let old = bindings(&settings()).unwrap();
        let key = "Ctrl+Shift+1".parse::<HotKey>().unwrap().id();
        let backend = Backend {
            keys: RefCell::new(old.keys().copied().collect()),
            ..Backend::default()
        };
        let mut registered = old.clone();
        let routes = Mutex::new(Routes {
            bindings: old,
            enabled: true,
            ..Routes::default()
        });
        routes
            .lock()
            .unwrap()
            .event(key, HotKeyState::Pressed, false);
        suspend_routes(&backend, &mut registered, &routes, true).unwrap();
        assert!(backend.keys.borrow().is_empty());
        assert!(registered.is_empty());
        assert!(
            !routes
                .lock()
                .unwrap()
                .event(key, HotKeyState::Released, false)
        );

        let mut edited = settings();
        edited.region_shortcut = "Alt+F10".into();
        let next = bindings(&edited).unwrap();
        sync_bindings(&backend, &mut registered, &next, true).unwrap();
        routes.lock().unwrap().bindings = next.clone();
        assert!(
            backend.keys.borrow().is_empty(),
            "editing must not reclaim grabs"
        );
        suspend_routes(&backend, &mut registered, &routes, false).unwrap();
        assert_eq!(*backend.keys.borrow(), next.keys().copied().collect());
        assert!(!backend.keys.borrow().contains(&key));
        assert!(!routes.lock().unwrap().suspended);

        // Settled calls must not clear a legitimate press before its release.
        let new_key = "Alt+F10".parse::<HotKey>().unwrap().id();
        routes
            .lock()
            .unwrap()
            .event(new_key, HotKeyState::Pressed, false);
        suspend_routes(&backend, &mut registered, &routes, false).unwrap();
        assert!(
            routes
                .lock()
                .unwrap()
                .event(new_key, HotKeyState::Released, false)
        );
    }

    #[test]
    fn restore_retains_in_flight_chords_but_failure_discards_them_without_waking() {
        struct BackendDuringRestore<'a> {
            routes: &'a Mutex<Routes>,
            chord: u32,
            release: bool,
            fail: bool,
        }
        impl Registration for BackendDuringRestore<'_> {
            fn register(&self, key: HotKey) -> Result<(), String> {
                if key.id() == self.chord {
                    let mut routes = self.routes.lock().unwrap();
                    assert!(routes.suspended && routes.restoring);
                    assert!(!routes.event(self.chord, HotKeyState::Pressed, false));
                    if self.release {
                        assert!(
                            !routes.event(self.chord, HotKeyState::Released, false),
                            "a complete chord cannot wake before restore commits"
                        );
                    }
                    if self.fail {
                        return Err("partial grab failed".into());
                    }
                }
                Ok(())
            }
            fn unregister(&self, _: HotKey) -> Result<(), String> {
                Ok(())
            }
        }
        let chord = "Ctrl+Shift+1".parse::<HotKey>().unwrap().id();
        for release in [false, true] {
            for fail in [false, true] {
                let routes = Mutex::new(Routes {
                    bindings: bindings(&settings()).unwrap(),
                    enabled: true,
                    suspended: true,
                    ..Routes::default()
                });
                let backend = BackendDuringRestore {
                    routes: &routes,
                    chord,
                    release,
                    fail,
                };
                let mut registered = Bindings::new();
                assert_eq!(
                    suspend_routes(&backend, &mut registered, &routes, false).is_err(),
                    fail
                );
                let mut state = routes.lock().unwrap();
                assert!(!state.restoring);
                assert_eq!(state.suspended, fail);
                if fail {
                    assert!(state.armed.is_empty() && state.pending.is_none());
                    assert!(registered.is_empty());
                    state.suspended = false;
                    assert!(!state.event(chord, HotKeyState::Released, false));
                } else {
                    if !release {
                        assert!(state.event(chord, HotKeyState::Released, false));
                    }
                    assert_eq!(state.pending.take(), Some(CaptureShortcut::Region));
                }
            }
        }
    }

    #[test]
    fn failed_suspend_and_resume_block_routing_until_successful_retry() {
        let desired = bindings(&settings()).unwrap();
        let key = *desired.keys().next().unwrap();
        let mut backend = Backend {
            keys: RefCell::new(desired.keys().copied().collect()),
            fail_cleanup: Some(key),
            ..Backend::default()
        };
        let mut registered = desired.clone();
        let routes = Mutex::new(Routes {
            bindings: desired.clone(),
            enabled: true,
            ..Routes::default()
        });
        assert!(suspend_routes(&backend, &mut registered, &routes, true).is_err());
        assert!(routes.lock().unwrap().suspended);
        assert!(
            !routes
                .lock()
                .unwrap()
                .event(key, HotKeyState::Pressed, false)
        );
        assert!(
            !routes
                .lock()
                .unwrap()
                .event(key, HotKeyState::Released, false)
        );
        assert!(routes.lock().unwrap().pending.is_none());
        // Focus can reverse after a failed release: resume the old, restored
        // registrations rather than remaining blocked behind a stale cache.
        suspend_routes(&backend, &mut registered, &routes, false).unwrap();
        assert!(!routes.lock().unwrap().suspended);
        assert_eq!(*backend.keys.borrow(), desired.keys().copied().collect());
        backend.fail_cleanup = None;
        suspend_routes(&backend, &mut registered, &routes, true).unwrap();
        backend.fail = Some(key);
        assert!(suspend_routes(&backend, &mut registered, &routes, false).is_err());
        assert!(backend.keys.borrow().is_empty());
        assert!(registered.is_empty());
        assert!(routes.lock().unwrap().suspended);
        // The opposite reversal is safe too: returning to Preferences after
        // failed restoration must keep every key released for the recorder.
        suspend_routes(&backend, &mut registered, &routes, true).unwrap();
        assert!(backend.keys.borrow().is_empty());
        backend.fail = None;
        suspend_routes(&backend, &mut registered, &routes, false).unwrap();
        assert_eq!(*backend.keys.borrow(), desired.keys().copied().collect());
        assert!(!routes.lock().unwrap().suspended);
        assert!(
            !routes
                .lock()
                .unwrap()
                .event(key, HotKeyState::Released, false)
        );
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
    fn failed_partial_release_restores_the_failed_chord_and_removes_additions() {
        let old = bindings(&settings()).unwrap();
        let changed = AppSettings {
            region_shortcut: "Alt+4".into(),
            window_shortcut: "Alt+5".into(),
            display_shortcut: "Alt+6".into(),
            ..settings()
        };
        let next = bindings(&changed).unwrap();
        let backend = Backend {
            keys: RefCell::new(old.keys().copied().collect()),
            fail_cleanup: old.keys().find(|id| !next.contains_key(id)).copied(),
            ..Backend::default()
        };
        assert!(
            rebind(&backend, &old, &next)
                .unwrap_err()
                .contains("release")
        );
        assert_eq!(*backend.keys.borrow(), old.keys().copied().collect());
    }

    #[test]
    fn failed_registration_reports_failure_to_clean_its_partial_grab() {
        let next = bindings(&settings()).unwrap();
        let failed = next.keys().next().copied();
        let backend = Backend {
            fail: failed,
            fail_cleanup: failed,
            ..Backend::default()
        };
        assert!(
            rebind(&backend, &Bindings::new(), &next)
                .unwrap_err()
                .contains("cleanup failed")
        );
    }

    #[test]
    fn rollback_attempts_every_added_key_even_after_a_cleanup_error() {
        let old = bindings(&settings()).unwrap();
        let mut changed = settings();
        changed.region_shortcut = "Alt+4".into();
        changed.window_shortcut = "Alt+5".into();
        changed.display_shortcut = "Alt+6".into();
        let next = bindings(&changed).unwrap();
        let keys: Vec<_> = next
            .keys()
            .filter(|key| !old.contains_key(key))
            .copied()
            .collect();
        let backend = Backend {
            keys: RefCell::new(old.keys().copied().collect()),
            fail: Some(keys[1]),
            fail_cleanup: Some(keys[0]),
        };
        assert!(
            rebind(&backend, &old, &next)
                .unwrap_err()
                .contains("cleanup failed")
        );
        assert_eq!(*backend.keys.borrow(), old.keys().copied().collect());
    }
}
