//! Opt-in scheduling diagnostics. Never log input contents or media paths.
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

static ENABLED: OnceLock<bool> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);

fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var_os("CAPTURES_NATIVE_TRACE").is_some())
}

pub fn event(name: &str, detail: impl FnOnce() -> serde_json::Value) {
    if enabled() {
        crate::emit(
            "native-trace",
            serde_json::json!({"name":name,
            "us":START.get_or_init(Instant::now).elapsed().as_micros(),
            "thread":format!("{:?}",std::thread::current().id()), "data":detail()}),
        );
    }
}

pub struct Span {
    name: &'static str,
    id: u64,
}
pub fn span(name: &'static str) -> Span {
    let id = if enabled() {
        NEXT.fetch_add(1, Ordering::Relaxed)
    } else {
        0
    };
    if id != 0 {
        event(name, || serde_json::json!({"span":id,"phase":"enter"}));
    }
    Span { name, id }
}
impl Drop for Span {
    fn drop(&mut self) {
        if self.id != 0 {
            event(
                self.name,
                || serde_json::json!({"span":self.id,"phase":"exit"}),
            );
        }
    }
}
