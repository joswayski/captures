//! Deliver worker results to the UI thread when they arrive, instead of
//! polling a channel from a timer: an open, idle window costs no wake-ups.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;

use gtk::glib;

type Handler = Rc<dyn Fn()>;

thread_local! {
    static HANDLERS: RefCell<HashMap<u64, Handler>> = RefCell::default();
}
static NEXT: AtomicU64 = AtomicU64::new(1);

/// Runs one UI-thread handler from any thread. Notifications that arrive
/// before the handler has run are coalesced into a single run.
#[derive(Clone)]
pub struct Wake {
    id: u64,
    pending: Arc<AtomicBool>,
}

impl Wake {
    /// Register `run` on the UI thread. It stays registered until [`remove`].
    pub fn new(run: impl Fn() + 'static) -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        HANDLERS.with(|handlers| handlers.borrow_mut().insert(id, Rc::new(run)));
        Self {
            id,
            pending: Arc::default(),
        }
    }

    /// Queue the handler on the main loop. Results a worker sends while its
    /// window is still being built are delivered once the loop next runs.
    pub fn notify(&self) {
        if self.pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let (id, pending) = (self.id, self.pending.clone());
        glib::MainContext::default().invoke(move || {
            // Cleared first: a result sent while the handler runs wakes again.
            pending.store(false, Ordering::Release);
            let handler = HANDLERS.with(|handlers| handlers.borrow().get(&id).cloned());
            if let Some(handler) = handler {
                handler();
            }
        });
    }

    /// Drop the handler (and whatever it owns); later notifications do nothing.
    pub fn remove(&self) {
        let handler = HANDLERS.with(|handlers| handlers.borrow_mut().remove(&self.id));
        drop(handler);
    }
}

/// A worker's result channel that wakes the UI thread on every send.
pub struct Outbox<T> {
    sender: mpsc::Sender<T>,
    wake: Wake,
}

impl<T> Outbox<T> {
    pub fn new(sender: mpsc::Sender<T>, wake: Wake) -> Self {
        Self { sender, wake }
    }

    pub fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        self.sender.send(value)?;
        self.wake.notify();
        Ok(())
    }

    /// Wake the UI thread for state shared outside the channel.
    pub fn wake(&self) {
        self.wake.notify();
    }
}
