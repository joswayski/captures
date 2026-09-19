//! One cancellable native capture at a time. The host owns this guard on its
//! native event-loop thread; workers carry only the generation, never OS handles.
use global_hotkey::{
    GlobalHotKeyManager,
    hotkey::{Code, HotKey},
};
use std::{
    marker::PhantomData,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Gate {
    next: AtomicU64,
    current: AtomicU64,
}

impl Gate {
    fn begin(&self) -> Result<u64, String> {
        // Even generations reserve the low bit for the irreversible commit point.
        let generation = self.next.fetch_add(2, Ordering::AcqRel) + 2;
        self.current
            .compare_exchange(0, generation, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| generation)
            .map_err(|_| "A capture is already in progress".into())
    }
    fn is_current(&self, generation: u64) -> bool {
        generation != 0 && self.current.load(Ordering::Acquire) & !1 == generation
    }
    fn shortcuts_allowed(&self, selector_generation: Option<u64>) -> bool {
        let current = self.current.load(Ordering::Acquire);
        match selector_generation {
            None => current == 0,
            Some(generation) => generation != 0 && generation & 1 == 0 && current == generation,
        }
    }
    fn cancel(&self, generation: u64) {
        let _ = self
            .current
            .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire);
    }
    fn commit(&self, generation: u64) -> bool {
        generation != 0
            && self
                .current
                .compare_exchange(
                    generation,
                    generation | 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
    }
    fn finish(&self, generation: u64) {
        self.cancel(generation);
        let _ =
            self.current
                .compare_exchange(generation | 1, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}

static GATE: Gate = Gate {
    next: AtomicU64::new(0),
    current: AtomicU64::new(0),
};

pub fn is_current(generation: u64) -> bool {
    GATE.is_current(generation)
}

pub fn cancel(generation: u64) {
    GATE.cancel(generation);
}

pub(crate) fn commit(generation: u64) -> bool {
    GATE.commit(generation)
}

pub(crate) fn shortcuts_allowed(selector_generation: Option<u64>) -> bool {
    GATE.shortcuts_allowed(selector_generation)
}

pub(crate) fn escape() {
    let generation = GATE.current.load(Ordering::Acquire) & !1;
    GATE.cancel(generation);
}

/// Uses a monotonic deadline, not a decrementing frame/timer counter. Late UI
/// callbacks cannot extend the countdown or round the final fraction down early.
#[derive(Clone, Copy)]
pub struct Countdown {
    deadline: Instant,
}

impl Countdown {
    pub fn new(now: Instant, seconds: u8) -> Self {
        Self {
            deadline: now + Duration::from_secs(seconds.into()),
        }
    }
    pub fn remaining(self, now: Instant) -> u8 {
        self.deadline
            .saturating_duration_since(now)
            .as_nanos()
            .div_ceil(1_000_000_000) as u8
    }
}

/// Temporary global Escape ownership. Construct, poll and drop on the AppKit or
/// winit event-loop thread. Session polling stops after the guard ends; Escape
/// hotkeys are released and the Windows low-level capture hook is disarmed.
/// Registration failure refuses the capture rather than silently losing Cancel.
pub struct CaptureFlow {
    generation: u64,
    countdown: Countdown,
    manager: GlobalHotKeyManager,
    _event_loop_thread: PhantomData<Rc<()>>,
}

impl CaptureFlow {
    pub fn begin(seconds: u8) -> Result<Self, String> {
        if seconds > 10 {
            return Err("Unsupported screenshot countdown".into());
        }
        let generation = GATE.begin()?;
        let registration: Result<_, String> = (|| {
            crate::shortcuts::install_dispatcher();
            let manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
            manager
                .register(HotKey::new(None, Code::Escape))
                .map_err(|e| e.to_string())?;
            captures_session::ensure_capture_escape_hook()?;
            captures_session::set_capture_escape_handler(Some(escape));
            captures_session::set_capture_escape_enabled(true);
            Ok(manager)
        })();
        match registration {
            Ok(manager) => {
                // Session queries can block (e.g. D-Bus); never run them on the
                // event-loop thread. This watcher exists only for this capture.
                std::thread::spawn(move || {
                    while GATE.is_current(generation) {
                        if !captures_session::capture_session_available() {
                            GATE.cancel(generation);
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                });
                Ok(Self {
                    generation,
                    countdown: Countdown::new(Instant::now(), seconds),
                    manager,
                    _event_loop_thread: PhantomData,
                })
            }
            Err(error) => {
                GATE.cancel(generation);
                Err(error)
            }
        }
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn countdown(&self) -> Countdown {
        self.countdown
    }
    /// Start the configured delay only after region confirmation. The same
    /// generation and Escape registration cover preparation, selection and delay.
    pub fn start_countdown(&mut self, seconds: u8) -> Result<(), String> {
        if seconds > 10 {
            return Err("Unsupported screenshot countdown".into());
        }
        if GATE.current.load(Ordering::Acquire) != self.generation {
            return Err("Capture is no longer pending".into());
        }
        self.countdown = Countdown::new(Instant::now(), seconds);
        Ok(())
    }
    pub fn is_current(&self) -> bool {
        GATE.is_current(self.generation)
    }
    pub fn cancel(&self) {
        GATE.cancel(self.generation);
    }
}

impl Drop for CaptureFlow {
    fn drop(&mut self) {
        GATE.finish(self.generation);
        captures_session::set_capture_escape_enabled(false);
        captures_session::set_capture_escape_handler(None);
        let _ = self.manager.unregister(HotKey::new(None, Code::Escape));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_boundaries_and_delayed_wakes_do_not_capture_early() {
        let start = Instant::now();
        let clock = Countdown::new(start, 3);
        for (milliseconds, expected) in [
            (0, 3),
            (999, 3),
            (1000, 2),
            (2499, 1),
            (2999, 1),
            (3000, 0),
            (12000, 0),
        ] {
            assert_eq!(
                clock.remaining(start + Duration::from_millis(milliseconds)),
                expected
            );
        }
        assert_eq!(
            clock.remaining(start + Duration::from_secs(3) - Duration::from_nanos(1)),
            1
        );
        assert_eq!(Countdown::new(start, 0).remaining(start), 0);
    }

    #[test]
    fn cancelled_worker_and_stale_cleanup_cannot_affect_a_new_capture() {
        let gate = Gate::default();
        assert!(!gate.is_current(0));
        let old = gate.begin().unwrap();
        assert!(gate.begin().is_err());
        assert!(gate.is_current(old));
        gate.cancel(old);
        let new = gate.begin().unwrap();
        assert!(!gate.is_current(old));
        gate.cancel(old);
        assert!(gate.is_current(new));
        gate.cancel(new);
        assert!(!gate.is_current(new));
    }

    #[test]
    fn shortcut_scope_requires_idle_or_the_exact_uncommitted_selector() {
        let gate = Gate::default();
        assert!(gate.shortcuts_allowed(None));
        assert!(!gate.shortcuts_allowed(Some(0)));
        let old = gate.begin().unwrap();
        assert!(!gate.shortcuts_allowed(None));
        assert!(gate.shortcuts_allowed(Some(old)));
        gate.cancel(old);
        assert!(!gate.shortcuts_allowed(Some(old)));
        let new = gate.begin().unwrap();
        assert!(!gate.shortcuts_allowed(Some(old)));
        assert!(gate.shortcuts_allowed(Some(new)));
        assert!(gate.commit(new));
        assert!(!gate.shortcuts_allowed(Some(new)));
        assert!(!gate.shortcuts_allowed(Some(new | 1)));
        assert!(!gate.shortcuts_allowed(None));
        gate.finish(new);
        assert!(!gate.shortcuts_allowed(Some(new)));
        assert!(gate.shortcuts_allowed(None));
    }

    #[test]
    fn cancellation_and_commit_have_one_unambiguous_winner() {
        let gate = Gate::default();
        let cancelled = gate.begin().unwrap();
        gate.cancel(cancelled);
        assert!(!gate.commit(cancelled));
        let committed = gate.begin().unwrap();
        assert!(gate.commit(committed));
        gate.cancel(committed);
        assert!(
            gate.is_current(committed),
            "saving cannot report cancellation after commit"
        );
        assert!(!gate.commit(committed), "a worker may persist only once");
        gate.finish(cancelled);
        assert!(gate.is_current(committed));
        gate.finish(committed);
        assert!(!gate.is_current(committed));
        assert!(gate.begin().is_ok());
    }
}
